# F5 follow-ups

Fixes and gaps found testing the merged Ansible rework. The specs carry the behaviour: INV (managed machines, the served lease), ANSRUN (claimed intent), ANSVAR (history, presentation).

Per-type variable scopes were split out to H5.

## Lease served, intent checked

The plugin takes a handed-over lease's intent from `CANOPY_INTENT` on its own command line, so a configure lease (no upgrade plan needed) can be run as an upgrade.

- [x] `AnsibleArgs` takes an optional `intent`; `for_group` refuses one that is not the lease's (409, naming both)
- [x] `AnsibleView` carries the lease: id, intent, playbook, limit, tags, skip tags
- [x] Rust tests for both; `just gen-openapi`
- [x] The run command passes only `CANOPY_URL` and `CANOPY_LEASE`: group, rank and intent come from the lease (`buildCommand` and its test)

Infra's plugin moves with this (it is being developed against this branch): with a lease, take group, rank, intent, playbook, limit and tags from what Canopy serves, and send `CANOPY_INTENT` as the claimed intent only where one is given.

## `managed_by_ansible` as a switch

- [x] Hold it to machine scope in `ansible_variables::set` beside `check_ansible_host` (the scope text editor saves through it too)
- [x] Import: drop the environment/group-wide `managed_wide` branch, which can no longer be set; a machine already switched on does not hold an import back
- [x] Migration carrying existing wider values onto machines
- [x] SPA: list every member; a switch per machine sets it true or false; an unmanaged machine is greyed and collapsed by default
- [x] Leave it out of the variable rows, the entry row's name completion, and the scope text editor

## Nesting

- [x] Environment indented beneath the group, machines beneath the environment

## History and undo

History is the audit log; nothing new stores plain values.

- [x] `AnsibleVariableSet` / `AnsibleVariableRemoved` detail gains the plain `value` (null for a secret), and `restored_from` (the entry restored) on a set
- [x] A query for one variable's history: set and removed entries naming the scope's record and a variable record of that name, newest first, within the database's retention
- [x] A query for the variables removed from a scope in the last week, not set again since

Earlier secret values:

- [x] Migration: `ansible_secret_values_held` (the audit entry that replaced or removed it, scope columns, name, `held_until`), one row per held value
- [x] Set over a secret, or removal of one, copies the old value to the scope's `<secret_name>-held` secret under the entry id before writing, inside the same commit-ahead as the change
- [x] Restore reads it back from there; restoring a secret holds the value it replaces in turn
- [x] Sweep in `jobs::ansible::held` (backups binary): forget priors past `held_until`, audited as Canopy's own action
- [x] Escrow: carry held priors beside the variables; write within the hour after a secret change (latest secret `updated_at` or prior row newer than the last write)

API and SPA:

- [x] `ansible_variables/history`, `ansible_variables/removed`, `ansible_variables/restore` (write grade, audited as a set)
- [ ] Undo offer after a removal (snackbar), restoring through `restore`
- [ ] History dialog from each row: who, when, value or "secret changed", restore where restorable
- [ ] Per scope: removed in the last week, each opening its history
- [ ] Rust tests (history query, restore plain, restore secret within the week, refused after, sweep), e2e for undo, history restore, removed list
