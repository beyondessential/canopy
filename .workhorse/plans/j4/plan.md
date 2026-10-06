# Retarget maintenance from the declare dialog

## Notes

- `IncidentDetail.tsx` opens the declare dialog with `scope="group"` and no `rank`, so declaring from an incident covers the whole group. MNT has it start at the incident's environment; the incident carries `rank`, so the default comes from there.
- Settling is derived per window (`window_end() + SETTLE` in `crates/database/src/maintenance_windows.rs`), and a window has one target. A move has to leave the old target settling while the window itself carries on elsewhere; see "Moves" below for the shape.
- A move must be refused server-side onto a target holding an open window of its own, as well as being unselectable in the dialog.
- Coverage marks in the dialog need the incident's failing issues with their scopes, checked against each candidate grain the way suspension resolves cover (application ⊂ machine ⊂ environment ⊂ group, group-scoped checks only under the group).
- The group page's split button loses its environment menu: one "Declare maintenance" control, the environment chosen in the dialog.
- The upgrade-plan declare (`Upgrades.tsx`) and the configuration run's "Declare the work" (`GroupInventorySection.tsx`) keep a fixed target.
- A move is the frontend's name for an amendment that changes the target; the backend has one amend operation, addressed by window, that can carry a new target. `declare` stays the upsert keyed by target for a fresh declaration.
- An amendment carries only the fields the operator changed, so retargeting onto another window leaves its end and note alone unless edited. The dialog tracks which fields were touched and shows the window's current end and note beside them.
- A window declared from an upgrade plan's offer records that plan, and amendment refuses a new target for it. The dialog amending such a window offers no other grain.
- A moved window appears in both targets' histories: the old target's over the span it covered there. History reads need the window's target changes (the amendment record) rather than only its current target columns.
- Amending a window to a new target is refused while a run lease is held against an environment it covers; the dialog shows that window's grain without the picker.

## Moves

The window row keeps its current target in `application_id` / `machine_id` / `server_group_id` / `rank`, so the one-open-window-per-target unique indexes, `open_for`, `open_over` (lease refusal), `list_open`, and `close_met_plans` keep reading it unchanged.
Each move appends a `maintenance_window_moves` row holding the target it left: the same four target columns under the same checks, `covered_from` (when the window started covering that target: its `declared_at`, or the previous move's `moved_at`), `moved_at`, `moved_by`, and `settled_at`.
A move row is a closed span of the window over a past target, so:

- **Suspension** reads it as settling-only until `moved_at + SETTLE`: `suspends()` and `suspended_targets()` add the move rows inside that cutoff to the suspended sets but never to the `holding_*` ones. Because the row keeps the grain it left, `SuspendedTargets` already draws the settling mark at that grain (group card settling, environment row held).
- **Settle sweep** claims move rows whose settle period has elapsed (`settled_at` stamped once) and re-evaluates their scope, the same way `claim_settled` does for ended windows. No Slack outbox row for either the move or its settle end.
- **History** for a target is the windows currently over it plus the windows whose move rows name it, each with the span it covered there.

The window's current span starts at its latest move's `moved_at`, or its `declared_at` where it never moved.

## Checklist

### Database

- [ ] `just migration maintenance_window_moves`: the `maintenance_window_moves` table above (FKs `ON DELETE CASCADE` like the window's, `num_nonnulls` = 1, rank needs a group, rank values), an index on `moved_at` where `settled_at IS NULL`, and per-target `(…, moved_at DESC)` indexes for history
- [ ] Same migration: `maintenance_windows.upgrade_plan_id UUID REFERENCES upgrade_plans (id) ON DELETE SET NULL`
- [ ] `just migrate` and pick up the regenerated `schema.rs`
- [ ] `MaintenanceWindowMove` model in `crates/database/src/maintenance_windows.rs`, re-exported from `lib.rs`; `upgrade_plan_id` on `MaintenanceWindow`
- [ ] `line_of_descent(db, scope, rank)`: the grains containing and contained by a starting target, nested group → environment → machine → application, pending machines under the group apart from environments, grains the start has none of passed over (machine in no group, pending machine). Used by both `amend` validation and the targets endpoint
- [ ] `covers(candidate scope + rank, issue scope)`: application ⊂ machine ⊂ environment ⊂ group, group-scoped checks under the group alone; written against the same reading `suspends()` uses so the dialog's marks and real suspension cannot disagree
- [ ] `MaintenanceWindow::amend(db, id, Amendment { target, expected_end, note }, by)`, every field optional (note tri-state): refuses an ended window; on a new target, refuses one off the line of descent, one with an open window (map the unique-index violation to `Conflict` for the race), one with `upgrade_plan_id` set, and one whose declarer or standing amender holds a live `InventoryLease` on an environment the window covers; writes the move row, the new target columns and `amended_by`/`amended_at`; re-evaluates the new scope's open issues with the "maintenance declared by" close reason
- [ ] `declare` takes `upgrade_plan_id`, stamped on insert only (amending through the plan's offer leaves an existing window movable)
- [ ] `suspends()` and `suspended_targets()` read move rows inside the settle cutoff as settling-only
- [ ] `claim_settled_moves` in `sweep()`, re-evaluating each claimed move's scope
- [ ] `list_for_scope` returns windows on the target and windows moved off it, each with `covered_from` / `covered_until` for this target

### Private server

- [ ] `maintenance/amend` (`write:`): `{ id, target?, rank?, expected_end?, note? }` → `MaintenanceWindow`; 404 for an unknown window, 400 off-line or ended, 409 for a target with its own window, a plan's window, or a leased window
- [ ] `maintenance/targets` (`read_only:`): `{ start target + rank, incident_id?, window_id? }` → the nested line of descent, each grain with its label, its open window (id, expected end, note, declarer), `choosable`, and, given an incident, whether it covers every effective failure in it. Given `window_id`, the response says whether that window can move at all (plan / lease) so the dialog can hide the picker
- [ ] Incident coverage reads the incident's current member issues with effective result failed and their scopes; reuse the incident detail's member read rather than a second query
- [ ] `DeclareArgs.upgrade_plan_id`, validated as an open plan on the declared environment
- [ ] `for_target` returns the history rows with their spans
- [ ] `just gen-openapi`, re-exports in `private-web/src/types.ts`

### Frontend

- [ ] `DeclareMaintenanceDialog`: a "Covers" select listing the targets response nested, each with kind and rank chips; the "not all failing checks" mark and the alert under the field when the chosen grain carries it; grains with their own window disabled while moving
- [ ] Dialog modes: declaring (fresh, or onto a grain's open window, which flips the title to "Amend maintenance" and shows "Already under maintenance until …"), amending (opened on a window; changing the grain makes the action "Move"); `fixed` for callers that cannot retarget, and when the targets response says the window cannot move
- [ ] Track touched fields; an amendment sends only those. When the dialog is on someone else's window, show its current end and note under the fields
- [ ] The explanatory text follows the chosen grain rather than the prop scope
- [ ] `IncidentDetail.tsx`: start at the incident's environment (`scope="group"` with `incident.rank`), pass the incident id; it opens as an amendment where that environment has its own window
- [ ] `MaintenanceSection.tsx`: replace the environment split-button menu with one "Declare maintenance" control; history rows for moved windows show the span they covered here
- [ ] `MaintenanceHeaderButton` component reading the target's open window, opening the dialog amending it (with `offerLift`) or declaring; placed in the action rows of `GroupDetail.tsx`, `MachineDetail.tsx`, and `ServerDetail.tsx` using each page's existing button style
- [ ] `Maintenance.tsx` fleet view: Amend opens the dialog able to move
- [ ] `Upgrades.tsx` declare-from-plan: `fixed`, sends `upgrade_plan_id`; `GroupInventorySection.tsx` "Declare the work": `fixed`
- [ ] `just typecheck`

### Tests

- [ ] `crates/database/tests/it/maintenance_windows.rs`: partial amendment leaves untouched fields; a move suspends the new target at once and leaves the old one settling for `SETTLE`, then the sweep re-evaluates it; move refused onto a target with its own window, off the line of descent, for a plan's window, and for a leased window (and allowed once the lease is released); history lists a moved window on both targets with its spans; no Slack outbox rows from a move or its settle; an issue in an open incident leaves it when a window moves over its target
- [ ] `crates/database/tests/it/upgrade_plans.rs`: a window declared from the plan holds it and cannot be moved; one declared earlier over the environment moves
- [ ] New `crates/private-server/tests/it/maintenance_amend.rs` (declared in `tests/it/main.rs`): amend endpoint statuses; the targets endpoint from each starting grain, including an application, a machine in no group, a pending machine, and a group with pending machines; coverage marks for a group-scoped failure in the headline environment's incident (only the group unmarked)
- [ ] `private-web/e2e/maintenance.spec.ts`: retarget from an incident with a group check failing; the header control on group, machine and application pages; moving a window from the fleet view; retargeting onto another operator's window shows its settings and leaves its note when untouched; plan declare has no picker. Extend `e2e/seed.ts` for move rows where a test needs a settling moved-off target
- [ ] `just test-package database`, `just test-package private-server`, `just test-e2e`, `cargo fmt`, no warnings
