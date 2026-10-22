# F5 follow-ups

Fixes and gaps found testing the merged Ansible rework. The specs carry the behaviour: INV (managed machines, the served lease), ANSRUN (claimed intent), ANSVAR (history, presentation).

Per-type variable scopes were split out to H5.

## Lease served, intent checked

The plugin takes a handed-over lease's intent from `CANOPY_INTENT` on its own command line, so a configure lease (no upgrade plan needed) can be run as an upgrade.

- [ ] `AnsibleArgs` takes an optional `intent`; `for_group` refuses one that is not the lease's (409, naming both)
- [ ] `AnsibleView` carries the lease: id, intent, playbook, limit, tags, skip tags
- [ ] Rust tests for both; `just gen-openapi`
- [ ] The run command stops passing `CANOPY_INTENT`, since a handed-over lease's intent comes from Canopy (`buildCommand` and its test)

Infra's plugin moves with this (it is being developed against this branch): take intent, playbook, limit and tags from the served lease, and send `CANOPY_INTENT` as the claimed intent only where one is given.

## `managed_by_ansible` as a switch

- [ ] Hold it to machine scope in `ansible_variables::set` beside `check_ansible_host` (the scope text editor saves through it too)
- [ ] Import: drop the environment/group-wide `managed_wide` branch, which can no longer be set
- [ ] SPA: list every member; a switch per machine sets or removes it; an unmanaged machine is greyed and collapsed by default
- [ ] Leave it out of the variable rows, the entry row's name completion, and the scope text editor

## Nesting

- [ ] Environment indented beneath the group, machines beneath the environment

## History and undo

History is the audit log; nothing new stores plain values.

- [ ] `AnsibleVariableSet` / `AnsibleVariableRemoved` detail gains the plain `value` (null for a secret), and `restored_from` (the entry restored) on a set
- [ ] A query for one variable's history: set and removed entries naming the scope's record and a variable record of that name, newest first, within the database's retention
- [ ] A query for the variables removed from a scope in the last week, not set again since

Earlier secret values:

- [ ] Migration: `ansible_secret_priors` (the audit entry that replaced or removed it, scope columns, name, `held_until`), one row per held value
- [ ] Set over a secret, or removal of one, copies the old value to the scope's `<secret_name>-prior` secret under the entry id before writing, inside the same commit-ahead as the change
- [ ] Restore reads it back from there; restoring a secret holds the value it replaces in turn
- [ ] Sweep in `jobs::ansible`: forget priors past `held_until`, audited as Canopy's own action
- [ ] Escrow: carry held priors beside the variables; write within the hour after a secret change (latest secret `updated_at` or prior row newer than the last write)

API and SPA:

- [ ] `ansible_variables/history`, `ansible_variables/removed`, `ansible_variables/restore` (write grade, audited as a set)
- [ ] Undo offer after a removal (snackbar), restoring through `restore`
- [ ] History dialog from each row: who, when, value or "secret changed", restore where restorable
- [ ] Per scope: removed in the last week, each opening its history
- [ ] Rust tests (history query, restore plain, restore secret within the week, refused after, sweep), e2e for undo, history restore, removed list
