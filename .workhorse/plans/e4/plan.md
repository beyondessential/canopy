# Remove plain IDs from human-readable text

## Diagnosis

The incident summary's location line ("Failed: Application reachability on 7fbdbfc3-…") went through `Application::label()`, which falls back from the name to the host to the UUID.
`Application::display_name()` already implements FLT#naming (name, else the type's sentence case) and says every surface flattening a name goes through it; `label()` and several inline `name.unwrap_or(id)` copies bypass it.
Machines have no type to fall back on, so every machine fallback lands on the UUID.

## Decisions

- An unnamed machine is not a state the system allows: a machine's name is mandatory and never blank.
  Prod (2026-10-05) has 0 of 113 live machines unnamed and 6 archived ones (all archived 2026-08-04, no hostname figure, no tailnet identity, unnamed applications).
  The migration backfills a null or blank name from the reported hostname, then the tailnet node name, then the first named application, then "Unnamed machine", and sets `NOT NULL` plus a non-blank `CHECK`.
- Incident short ids ("Incident 1a2b3c4d on …") stay as they are: they are a reference handle, not a fallback name.
- Not-found errors that echo back the id the caller sent stay: there is no record to name, and the id is the caller's own input.
- Machine name is not in the public API, so this is a private-API change only.

## Machine name mandatory

- [x] FLT naming: a machine always has a name
- [x] Migration: backfill, `NOT NULL`, non-blank `CHECK`
- [x] `Machine.name: String`, `NewMachine.name: String`, `MachineUpdate.name: Option<String>`
- [x] Private API: create requires a non-blank name, update can rename but not clear; 400 on blank
- [x] Drop every `machine.name` fallback in Rust
- [x] Test fixtures and seed insert machines with a name
- [x] SPA: create/edit forms require a name; drop `?? "Unnamed machine"` and `name ?? id` fallbacks
- [x] `just gen-openapi`

## Display-name sweep (backend)

- [x] Delete `Application::label()`; reachability message and incident summary use `display_name()`
- [x] Delete `statuses::machine_label` and `backup::staleness::machine_label`; `server_label` keeps its host qualifier over `display_name()`
- [x] Incident summary: no UUID when a location's record wasn't loaded
- [x] Maintenance target label, certificate forgotten-pause self-alert, reconcile labels
- [x] Private-server display fields: application breadcrumb name, upgrade plan failed test, inventory host/application names, inventory lease 409
- [x] `Application::names_by_ids` returns display names, so the issues API and MCP stop re-deriving (or skipping) the fallback
- [x] Backup recent runs and restore checks carry their machine's name, so a box that has left the group still reads by name
- [x] Group ids in prose: rotation-broken alert, domain overlap 409, group archive 409, backup bucket clash 409
- [x] Identity id in the attach-tailnet-device 409
- [x] Slack delivery failure self-alert names the incident's target, not row/incident ids
- [x] Tailnet key-expiry sweep drops the Tailscale node id beside the node name

## Display-name sweep (SPA)

- [x] Device names fall back to tailnet name, never the device id (`DeviceShorty`, `MachineIdentitySection`, device page title)
- [x] Fleet figures, backup panel, group inventory, migration tests, restore replicas/consumers: no id fallbacks
- [x] Restore replica suggested name never carries an id
- [x] Restore checks' machine column shows the machine's name
- [x] Playwright coverage: machine edit refuses a blank name, device naming, run/restore machine names, unnamed application in migration tests
