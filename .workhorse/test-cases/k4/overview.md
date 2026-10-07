# K4: cron backup schedules

## Resolution

- [x] A machine override wins over a group override, which wins over the fleet default (verifies spec: BKO)
- [x] Clearing a layer falls back to the next one, and the view names the layer in force (verifies spec: BKO)
- [x] A machine override survives the machine moving group and the type being disabled then re-enabled (verifies spec: BKO)
- [x] Every set and clear is in the layer's history with who and when (verifies spec: BKO)

## Cron grammar and validation

- [x] Names, ranges, steps, lists, `L`, `15W`, `5L`, `5#3`, and Sunday as 0 or 7 are accepted (verifies spec: BKO)
- [x] Vixie day-field semantics: `0 12 * * 2` fires on Tuesdays only, `0 12 1-31 * 2` daily (verifies spec: BKO)
- [x] `H` alone is accepted; `H/15`, `H(0-5)` and `H` inside a list are refused (verifies spec: BKO)
- [x] `H` resolves to the same values for the same machine and type every time, and differently across machines (verifies spec: BKO)
- [x] An expression that never fires (e.g. `0 0 30 2 *`, or `0 0 H 2 *`) is refused with a reason (verifies spec: BKO)
- [x] An expression with firings under an hour apart (e.g. `*/30 * * * *`) is refused with a reason (verifies spec: BKO)
- [x] An expression carrying a timezone field is refused
- [x] An interval under an hour is refused (verifies spec: BKO)

## Timezones

- [x] A schedule's own zone wins over the machine's reported zone (verifies spec: BKO)
- [x] A fleet default of `0 2 * * *` fires at each machine's own 2am (verifies spec: BKO)
- [x] A reported Windows zone name is read as its IANA equivalent (verifies spec: BKO)
- [x] A machine with no reported zone, or an unrecognised one, falls back to UTC and is flagged on the group view, the machine page, and the preview (verifies spec: BKO)
- [x] Spring-forward: a firing in the skipped hour doesn't happen that day (verifies spec: BKO)
- [x] Fall-back: a daily firing in the repeated hour happens once; an hourly expression skips the repeated wall-clock hour and keeps going (verifies spec: BKO)

## Due windows

- [x] Under cron, the backup is due from the firing (plus spread offset when there's no `H`) until halfway to the next firing (verifies spec: BKO)
- [x] A failed run within the window is retried; a success within the window ends it (verifies spec: BKO)
- [x] A window that closes unmet isn't caught up later; the backup waits for the next firing (verifies spec: BKO)
- [x] A machine starting to participate mid-window is due at once (verifies spec: BKO)
- [x] Changing a schedule at 14:00 from `0 2 * * *` to `0 6 * * *` doesn't make the backup due until the next 06:00 (verifies spec: BKO)
- [x] A machine reporting a new zone doesn't become due from a firing before the change (verifies spec: BKO)
- [x] A change to a layer the machine's schedule doesn't resolve through doesn't move its effective moment
- [x] Interval and manual-only behave as before, including on-demand requests (verifies spec: BKO)

## Staleness

- [x] Under cron, one missed firing is not stale and two consecutive missed firings are (verifies spec: BKJ)
- [x] Firings before the schedule took effect don't count (verifies spec: BKJ)
- [x] A machine already stale when a cron schedule takes effect stays stale until it backs up (verifies spec: BKJ)
- [x] A never-backed-up machine is stale once two firings have closed since its expectation began (verifies spec: BKJ)
- [x] Manual-only is never stale (verifies spec: BKJ)

## Editing and display

- [x] The fleet default and group override editors offer manual-only, interval and cron (verifies spec: BKO)
- [x] The machine override editor appears on the group's backup view and the machine page, showing the inherited schedule when unset (verifies spec: BKO)
- [x] The preview lists the next few firings with `H` resolved, per machine for an override and one machine per distinct zone for a group override or default (verifies spec: BKO)
- [x] A refused expression is reported as it is typed, with the reason (verifies spec: BKO)
- [x] Next backup shows the next firing, "due until" while a window is open, the interval's next due time or "due now", and "manual" (verifies spec: BKO)
- [x] All schedule set and clear actions work in write mode; clearing a retention override still needs danger (verifies spec: SAFE)
- [x] Get machine and Get group in the fleet query interface carry effective schedules, next backups, and overrides (verifies spec: MCP)
