# K4: cron backup schedules

## Technical notes

- Scheduling stays pull-based: `backup_now` on the `alertd` status response keeps its shape, so the agent and the public API are untouched. All the change is in `backups_due_now_for_machine` (`crates/commons-servers/src/backup_jobs.rs`) and what feeds it.
- Schedule storage: the fleet default (`backup_type_defaults`) and the group override (`server_group_backup_schedule`) each gain a cron expression and an optional zone beside `expected_interval`, with a CHECK that at most one of interval and cron is set (neither means manual-only). Add a new `(machine_id, type)` override table with the same three columns and `ON DELETE CASCADE` from machines. Retention stays off the machine table.
- Resolution order: machine override, then group override, then fleet default. Extend `effective_interval` into an `effective_schedule` returning an enum (Manual / Interval / Cron { expr, zone }), and make staleness, inspection's window and the group view use it.
- Zone: the schedule's zone, else the machine's reported `osTimezone` figure, else UTC. Use `jiff` (already a dependency) for zone maths. Validate zone names with `TimeZone::get` the way upgrade plans do. A zone is only offered and stored on cron schedules.
- Windows zone names: map the reported `osTimezone` through the CLDR `windowsZones` table (territory `001`) before `TimeZone::get`. Look in `~/.cargo/registry/src` for a crate carrying the table before vendoring it. Anything still unresolved falls back to UTC with the flag, reason "unrecognised" as distinct from "unreported".
- Cron parsing: `cronexpr` 1.7 (add with `cargo add`; it's built on `jiff`). Its grammar is what BKO specifies: Vixie day-field semantics, names, `L`/`W`/`5L`/`5#3`, and `H` as a lone value via `ParseOptions::hashed_value`, mapped per field by `hash % field_range_len`. Seed `hashed_value` from a stable hash of `(machine_id, type)`, reusing the FNV mixing in `jitter_slot_in`. Store the five fields without a zone and append the resolved zone at parse time; refuse an operator expression that carries a sixth (zone) field, since the zone is its own setting.
- Validation of the "never fires" and "hourly floor" rules has to range over every `H` resolution. `H` is substituted per field by Canopy rather than through `cronexpr`'s `hashed_value`, so each field resolves independently. Only the day and month fields decide whether an expression fires, so those are enumerated over; the hourly floor reduces to the minute field resolving to a single value. Validation ignores DST entirely (naive wall clock).
- Runtime filter over `cronexpr`'s firings: drop any firing less than an hour after the previous kept one, or whose civil (wall-clock) datetime equals the previous kept one's (the fall-back duplicate `cronexpr` emits for a repeated hour). A dropped firing opens no window, doesn't bound the previous window, and isn't a staleness opportunity. Spring-forward needs nothing: `cronexpr` already omits firings in the skipped hour. A 65-minute floor was considered and rejected because it would drop every firing of an hourly expression.
- Spread offset for non-`H` expressions: a stable hash of `(machine_id, type)`, capped at the smaller of 15 minutes and a quarter of the due window.
- Due window: from the firing (plus offset) to the midpoint between this firing and the next one.
- Staleness under cron: stale when the two most recent firings with closed windows have both gone without a success since the earlier one.
- Escrow: the recovery vault writer (`crates/jobs/src/backup/recovery_snapshot.rs`) carries the fleet defaults, each group's overrides (with cron and zone, on the group's own rows), the machine overrides and the schedule history, and its schema version moved from 2 to 3.
- UI: extend `TypeDefaultEditor` (`BackupDefaults.tsx`) and `OverrideEditor` (`BackupPanel.tsx`) with a three-way kind selector. Reuse the timezone `Autocomplete` from `Upgrades.tsx`. Previews need a private endpoint that resolves the next firings for a given expression and machine(s). Add the machine override editor to the group panel's per-machine rows and to the machine page.
- Run `just gen-openapi` for the private API changes. Add Playwright coverage for the editors.
- Schedule history: each layer's set/clear is appended to a history (layer key, kind, timing, zone, who, when) rather than overwriting in place; the current row per layer is the latest. Shown beside each editor.
- Effective-since per `(machine, type)`: the latest history entry on the resolution path that changed the resolved schedule (a change to a shadowed layer doesn't count; clearing a higher layer does), or the moment the machine's reported `osTimezone` last changed when the zone comes from the machine. Record that zone-change moment on ingest rather than reading it back out of the status history.
- Due and staleness stay computed on each push from schedule, zone and effective-since; firings before effective-since open no window and don't count towards staleness. Firings and windows themselves are still never persisted.
- Staleness carry-over when a cron schedule takes effect: an open staleness issue for the `(machine, type)` stays raised until a successful backup (or the type goes manual-only), read from the existing check-state rather than recomputed.
- The UTC-fallback flag needs the effective-schedule resolution to report where its zone came from (schedule, machine, or fallback), so the group view, machine page and preview endpoint can all show it.
- Interval floor: the UI can't set a sub-hour interval, but add a migration raising any `expected_interval`/`default_interval` under an hour to one hour so the floor holds in the data too, and enforce it server-side in the set handlers.
- Inspection cadence: `effective_interval_for_group` takes the minimum interval floored up to weekly, so it's always at least weekly. Cron types contribute nothing to it and the weekly floor applies.
- Machine overrides are keyed by machine, so they survive a group move and a type being disabled with no extra code. Don't cascade them from capabilities or group membership.
- Safety grades: every schedule handler is `write`. Today `clear_schedule` is `danger` and deletes the whole `server_group_backup_schedule` row, retention override included. Split it so clearing the schedule part is `write` and clearing a retention override keeps its own (danger) grade, since shortening retention destroys snapshots at the next maintenance.
- Fleet query interface (MCP): Get machine gains each type's effective schedule, layer, and next backup; Get group gains the machines' overrides. Overdue in Find backup problems comes from staleness, so it follows cron for free.

## Checklist

- [x] Add `cronexpr` with `cargo add`; schedule enum, parser wrapper (zone appended, sixth field refused, `H` seeded), validation, runtime firing filter, unit tests incl. Pacific/Auckland DST transitions
- [x] Migrations: cron + zone columns on defaults and group overrides with the at-most-one CHECK; machine override table; schedule history; machine zone-changed-at; sub-hour interval migration
- [x] Windows zone mapping and zone-source reporting (schedule / machine / unreported / unrecognised)
- [x] `effective_schedule` resolution with layer, zone source, and effective-since
- [x] Due computation in `backups_due_now_for_machine`: interval as today, cron windows with spread offset, mid-window arrival
- [x] Staleness under cron, counting only firings since effective-since, with carry-over of an open staleness issue
- [x] Private API: set/clear for each layer (all `write`), history reads, firing preview endpoint, next-backup per machine; split retention clearing from schedule clearing; `just gen-openapi`
- [x] UI: kind selector on fleet default and group override editors, machine override editor on the group view and machine page, zone autocomplete, live preview and refusal reasons, history, next backup display, UTC flag
- [x] MCP: effective schedule and next backup on Get machine, overrides on Get group
- [x] Playwright coverage for the editors and preview
