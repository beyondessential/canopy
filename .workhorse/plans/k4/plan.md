# K4: cron backup schedules

## Technical notes

- Scheduling stays pull-based: `backup_now` on the `alertd` status response keeps its shape, so the agent and the public API are untouched. All the change is in `backups_due_now_for_machine` (`crates/commons-servers/src/backup_jobs.rs`) and what feeds it.
- Schedule storage: the fleet default (`backup_type_defaults`) and the group override (`server_group_backup_schedule`) each gain a cron expression and an optional zone beside `expected_interval`, with a CHECK that at most one of interval and cron is set (neither means manual-only). Add a new `(machine_id, type)` override table with the same three columns and `ON DELETE CASCADE` from machines. Retention stays off the machine table.
- Resolution order: machine override, then group override, then fleet default. Extend `effective_interval` into an `effective_schedule` returning an enum (Manual / Interval / Cron { expr, zone }), and make staleness, inspection's window and the group view use it.
- Zone: the schedule's zone, else the machine's reported `osTimezone` figure, else UTC. Use `jiff` (already a dependency) for zone maths. Validate zone names with `TimeZone::get` the way upgrade plans do.
- Cron parsing: there's no cron crate in the tree yet, and `H` (Jenkins hash syntax) is unlikely to be supported upstream. Check `croner` and similar crates locally before deciding. A small in-house 5-field parser with `H` seeded from a hash of `(machine_id, type)` may be the cleaner choice, reusing the FNV mixing in `jitter_slot_in`.
- Validation of the "never fires" and "hourly floor" rules has to range over every `H` resolution. Bound it by checking the expression's firing set over a full year for the extreme `H` values per field, or by reasoning per field. Decide during implementation.
- Spread offset for non-`H` expressions: a stable hash of `(machine_id, type)`, capped at the smaller of 15 minutes and a quarter of the due window.
- Due window: from the firing (plus offset) to the midpoint between this firing and the next one.
- Staleness under cron: stale when the two most recent firings with closed windows have both gone without a success since the earlier one.
- Maintenance and rotation avoidance: "busy" means any machine in the group has a backup in progress (issued credentials, still valid, no report) or an open, unmet cron window. The deferral bound reuses the existing `slot_deadline_due` window: a job only defers while it can still run within its current window.
- Escrow: include the machine override table and the new columns.
- UI: extend `TypeDefaultEditor` (`BackupDefaults.tsx`) and `OverrideEditor` (`BackupPanel.tsx`) with a three-way kind selector. Reuse the timezone `Autocomplete` from `Upgrades.tsx`. Previews need a private endpoint that resolves the next firings for a given expression and machine(s). Add the machine override editor to the group panel's per-machine rows and to the machine page.
- Run `just gen-openapi` for the private API changes. Add Playwright coverage for the editors.
- Schedule history: each layer's set/clear is appended to a history (layer key, kind, timing, zone, who, when) rather than overwriting in place; the current row per layer is the latest. Shown beside each editor.
- Effective-since per `(machine, type)`: the latest history entry on the resolution path that changed the resolved schedule (a change to a shadowed layer doesn't count; clearing a higher layer does), or the moment the machine's reported `osTimezone` last changed when the zone comes from the machine. Record that zone-change moment on ingest rather than reading it back out of the status history.
- Due and staleness stay computed on each push from schedule, zone and effective-since; firings before effective-since open no window and don't count towards staleness. Firings and windows themselves are still never persisted.
- Staleness carry-over when a cron schedule takes effect: an open staleness issue for the `(machine, type)` stays raised until a successful backup (or the type goes manual-only), read from the existing check-state rather than recomputed.
- The UTC-fallback flag needs the effective-schedule resolution to report where its zone came from (schedule, machine, or fallback), so the group view, machine page and preview endpoint can all show it.
