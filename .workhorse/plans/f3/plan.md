# Re-grading a check closes the incidents it leaves without a failure

## Approach

A check state stores its effective result at filing time, and nothing re-grades it until the check reports again.
Instance silences already re-grade stored states (`regrade_instanced_states`), replaying the instances and the inputs their rules read.
This card generalises that to every check state and runs it on every fleet policy save.

A plain check's state keeps no record of the report fields its rules read (`grading_context` is only stored with instances), so a faithful replay needs it stored for every filing.
States filed before that lands fall back to an empty `status.*` and the target's current tags until their next report.

A device-reported check's headline and message are worded from its grade in the public server.
That wording moves into the database crate so a re-grade words the state as a report with the same grade would.
Canopy's own checks keep their filer's message while still in trouble, since it describes the observation and so is still accurate; graded out of trouble a plain one is worded as a reported check would be, and instanced ones take the generic instance message, as an instance silence's re-grade already does. The choice lives in `ReportWording::regraded`.

A policy change's re-grade carries the saving operator through to membership, so an incident it closes is credited to them; an instance silence's re-grade stays unattributed, as whole-check silences are.

Membership gets two fixes independent of policy:
a member whose failure ends without leaving (failed to warning) now lets the incident close or linger once no effective failure is left, and a re-grade that makes a live member's failure escalating escalates the incident once.

A state filed before plain checks kept their inputs is re-graded only where no rule reads `status.*`, since graded as if its report had none a rule that held its failure would stop matching; the target's tags are loaded for it only when a rule reads them.

When several states settle at once (a push, a re-grade, the deferred re-evaluation queue), failures settle first, then warnings, then recoveries (`settle_order`): a failure lessening in place now ends it, so a fresh failure has to join before the lessened one settles or an incident with no linger window closes and reopens.
A re-grade takes every target lock it will need up front, in group order, so two concurrent saves over overlapping targets cannot take them in opposite orders.

A broken state retaining a failure keeps it through a re-grade, because the state does not record which definite result it retained; its next report settles it.

## Checklist

- [x] Membership: failure ending in place closes (operator re-grade) or lingers (report) when no effective failure remains; share the close-or-linger tail with `leave_open_incident`
- [x] Membership: a re-grade making a live member's failure escalating escalates as a join would
- [x] Store `grading_context` for plain check states too (migration relaxing `issues_instances_graded_together`; split `CheckStateStamp::instanced` into instances and inputs)
- [x] Move device health-check wording into the database crate and use it from public-server
- [x] `regrade_check_states` for one catalog entry, sharing the grading core with `regrade_instanced_states`; only changed states are written and re-evaluated
- [x] Run it from `CheckPolicy::update` and `CheckPolicy::update_rules` in the same transaction as the save
- [x] Tests (database and public-server crates) and test-cases file
- [x] Settle failures first everywhere several states settle together; lock a re-grade's targets up front; leave legacy states a status rule reads; skip resolved states
