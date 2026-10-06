# Retarget maintenance from the declare dialog

## Notes

- `IncidentDetail.tsx` opens the declare dialog with `scope="group"` and no `rank`, so declaring from an incident covers the whole group. MNT has it start at the incident's environment; the incident carries `rank`, so the default comes from there.
- Settling is derived per window (`window_end() + SETTLE` in `crates/database/src/maintenance_windows.rs`), and a window has one target. A move has to leave the old target settling while the window itself carries on elsewhere, so the move needs something that holds a settle period for the uncovered part without being a window in that target's history. Work out the shape before building the move.
- A move must be refused server-side onto a target holding an open window of its own, as well as being unselectable in the dialog.
- Coverage marks in the dialog need the incident's failing issues with their scopes, checked against each candidate grain the way suspension resolves cover (application ⊂ machine ⊂ environment ⊂ group, group-scoped checks only under the group).
- The group page's split button loses its environment menu: one "Declare maintenance" control, the environment chosen in the dialog.
- The upgrade-plan declare (`Upgrades.tsx`) and the configuration run's "Declare the work" (`GroupInventorySection.tsx`) keep a fixed target.
