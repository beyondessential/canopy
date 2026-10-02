# Instanced checks and instance silences

Scenarios verifying that reported checks can carry instances, that a check carries its own detail object, and that one instance can be silenced on a target.

## Push shape

- [ ] A check with fields in a nested `detail` object is accepted and its fields are recorded as the check's detail (verifies spec: STA)
- [ ] A plain check with flat fields beside `check`/`result` is still accepted and read the same as the nested form (verifies spec: STA)
- [ ] A plain check carrying flat fields and `detail` together is refused (verifies spec: STA)
- [ ] An instanced check with a flat field beside `instances` is refused (verifies spec: STA)
- [ ] A `detail` that is not an object, on a check or an instance, is refused (verifies spec: STA)
- [ ] A check carrying both `result` and `instances` is refused (verifies spec: STA)
- [ ] A check carrying none of `result`, `healthy`, `instances` is refused
- [ ] An instance with an empty key is refused (verifies spec: STA)
- [ ] An instance without a `result` is refused (verifies spec: STA)
- [ ] An instance with `healthy` instead of `result` is refused
- [ ] An instance reporting `broken` is refused (verifies spec: STA)
- [ ] The push response answers an instanced check once, with the check's policy (verifies spec: STA)
- [ ] Status history records an instanced push verbatim, `instances` included

## Grading

- [x] An instanced check's effective result is the most urgent instance that was not skipped (verifies spec: CHK)
- [x] An instanced check whose instances are all skipped is skipped (verifies spec: CHK)
- [ ] An instanced check's message is written by Canopy and names its degraded instances by label, falling back to the key (verifies spec: CHK)
- [ ] An instance absent from the next push has recovered (verifies spec: CHK)
- [ ] `instances: {}` recovers every instance the check held (verifies spec: CHK)
- [ ] A check that switches between plain and instanced across pushes stays one state (verifies spec: STA)
- [ ] One catalog rule on `check.<field>` grades a check's plain form and its instanced form alike (verifies spec: CHK)
- [x] A rule reads an instance's field over the check's shared field of the same name (verifies spec: CHK)
- [x] A rule evaluated for an instance reads that instance's result as `check.result` (verifies spec: CHK)
- [ ] A rule evaluated for a reported instance sees the push's report fields (`status.*`) and the target's tags (verifies spec: CHK)
- [x] A rule pinning a field only some instances carry grades only those instances (verifies spec: CHK)
- [x] A plain check whose own fields include `instances`, `degraded` and `total` is still a plain check

## Brokenness

- [x] A check reported `broken` with no instances, after reporting instances, presents every held instance as broken (verifies spec: CHK)
- [x] A broken instanced check retains its last definite result (verifies spec: CHK)
- [x] A broken instanced check recovers none of its instances; the next healthy push grades them afresh (verifies spec: CHK)

## Instance silences

- [x] Silencing one instance on an application quiets that instance only; the check is graded on the rest (verifies spec: CHK)
- [x] Silencing every degraded instance leaves the check graded on its passing instances, or skipped when none remain (verifies spec: CHK)
- [x] A group-scoped instance silence quiets that key on every application in the group reporting the check (verifies spec: CHK)
- [x] A machine-scoped instance silence quiets that key on a machine check (verifies spec: CHK)
- [x] Silencing an instance re-grades the check immediately, without waiting for the next push, and an incident whose last failure was that instance closes (verifies spec: CHK, INC)
- [x] Unsilencing an instance re-grades the check immediately
- [x] A re-grade after an instance silence reads the report fields and tags the last filing graded with, so no rule's inputs change (verifies spec: CHK)
- [x] A check silenced out of trouble and brought back by unsilencing presents the title its last filing gave it
- [x] An instance silence and a whole-check silence on the same check and target coexist
- [x] The reporting source is told the check's policy, unaffected by instance silences (verifies spec: CHK, STA)
- [ ] A target's silences list shows the instance's label and key, and marks a silence whose key the check no longer reports (verifies spec: CHK)

## Canopy's own instanced checks

- [ ] Backup staleness and reconciliation instances are keyed by backup type
- [ ] Restore-verification, migration-test and redaction instances are keyed by replica type, intent and declared name; two replicas of one type and intent are told apart (verifies spec: RST)
- [ ] An instance silence on a backup-type instance quiets that type only
- [ ] Reporting-schema instances are keyed by version
- [ ] Relay substrate instances carry keys; a relay check that cannot be read is filed broken at check level (verifies spec: K8S)
- [ ] An instanced substrate check's message is Canopy's, not the relay's

## Operator interface

- [ ] A target's checks list an instanced check's degraded and silenced instances, each with result, label and its own silence control, and count its passing ones (verifies spec: CHK)
- [ ] Silencing an instance from the target's checks, for the target and for the group (verifies spec: CHK)
- [ ] An issue for an instanced check offers silencing the whole check or one of its degraded instances, from within an incident (verifies spec: CHK)
- [ ] The silenced refs section shows an instance silence and marks it "not reported" when its key is gone (verifies spec: CHK)
- [ ] The rule-authoring sample for an instanced check shows one instance's fields merged over the shared ones (verifies spec: CHK)
- [ ] The fleet-wide check page offers no instance silence
