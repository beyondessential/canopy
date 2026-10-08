# Re-grading a check closes the incidents it leaves without a failure

## Policy change re-grades at once

- [x] Lowering a reported check's ceiling to warning closes its incident immediately and words the state as a warning (verifies spec: CHK, INC)
- [x] Lowering the ceiling to passed recovers the state and closes the incident without lingering (verifies spec: CHK, INC)
- [x] A rule change re-grades a plain check from the report fields its last report carried (verifies spec: CHK)
- [x] Reviewing a pending check whose observed failure was capped at warning opens an incident (verifies spec: CHK, INC)
- [x] A policy change re-grades every target's state of one of Canopy's own checks, keeping Canopy's message (verifies spec: CHK)
- [x] Raising a ceiling back to failed grades the failures back in and opens a fresh incident (verifies spec: INC)
- [x] Saving a policy that changes no grade leaves the states as they were
- [ ] A policy change on one application type's check leaves another type's same-named check alone (verifies spec: CHK)
- [ ] Saving a policy in the healthchecks admin UI closes an open incident on the incidents page

## Membership

- [x] A report lessening the last failure to a warning lingers the incident, and the failure returning ends the lingering (verifies spec: INC)
- [x] Making a live failure escalating escalates its notified incident once (verifies spec: INC)
- [x] An incident with another live failure stays open when one failure is graded down to a warning (verifies spec: INC)
