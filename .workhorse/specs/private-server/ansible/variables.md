---
id: ANSVAR
---

# Ansible variables

The variables an Ansible run is served, set on a group, an environment, or a machine, and the catalogue that describes the ones the playbooks read (see [INV](overview.md)).

## Variables

A variable has a name, and that name is unique per group, per environment, and per machine.
What a run is served merges those three scopes name-wise, a machine's value over its environment's over its group's.

A value is JSON and is served as it was stored.

A name can be prefixed with an application's type, which presents the variable against that application in the operator interface.
The data model and the API scope it no differently.

A variable can be marked as containing a secret, which applies to the whole value rather than to a part of it.
A secret's value is held in Canopy's secret store (see [BKO](../backup.md)) rather than beside its name, and is never logged.
Only an administrator can read, set, or replace the value of a secret variable, or remove one.
Non-admins can list secret variables by name only.
Variables must expose whether they are secret over the API; consumers should avoid caching or displaying them if so.
Variables are backed up in the escrow store (see [ESC](../../jobs/escrow.md)) to survive the loss of the secret store.

## Catalogue

The variables the playbooks read are described by a catalogue the infra repository publishes on its main branch, at `ansible/variables.schema.json`.
It is a JSON Schema of an environment's variables, each with its type, its default, what it means, and annotations under `x-canopy`: whether it is secret, the scopes it may be set at, the application types it configures, and whether it is reserved to the plugin.
The same file lists the playbooks a run can start, each with the intent it is run with, the application types it configures, and the tags it takes (see [ANSRUN](runs.md)).
Canopy reads it from the main branch, and keeps using the last copy that read as a catalogue while the repository cannot be reached or publishes one that does not.
Either lasting raises a self-alert (see [SELF](../self-alerts.md)).

A variable the catalogue describes is held to it.
Its value must match its type, it can be set only at the scopes the catalogue allows, and it is secret exactly when the catalogue says so.
A reserved variable cannot be set, since a run sets it for itself.
A name the catalogue does not describe can be set freely, so a variable a playbook has just started reading is not held up by the catalogue.
A value set before the catalogue described it, or before the catalogue changed, stays as it is and is marked where it no longer matches.
The exception is a value the catalogue comes to call secret, which Canopy moves into the secret store when it reads that catalogue, so a secret is never held beside its name.
The application types a variable configures decide where it is offered (see "Presentation"), and are not held to, since a machine is configured before its applications arrive.

## Audit

Setting or removing a variable is audited (see [AUD](../../platform/audit-log.md)).
A change in the catalogue Canopy reads is audited as Canopy's own action, naming the version read, since it changes what a variable is held to, and so is each value it moves into the secret store.
A secret's value never appears in a log.

## Presentation

An environment with variables set presents them collapsed, by count, so they take no room until asked for.
An environment with none, on itself or its machines, offers adding its first beside an import (see [ANSIMP](import.md)).

Expanded, the variables are listed by scope: the group's, the environment's, then each machine that sets anything of its own.
A last row adds a scope for a member that sets nothing yet.
A value overriding one from a wider scope is marked with the scope it overrides.
A secret variable appears by name, with when it last changed, and never its value.

Each scope ends with a row for adding a variable, driven from the keyboard: its name completes from the catalogue, showing each known variable's type, default, and meaning, and its value is entered in a form shaped by that type.
Setting a variable leaves the operator in a fresh row of the same scope, so several are entered in a run of typing.
A value the catalogue types as an object is entered as a field for each property it names, each labelled, required where the catalogue requires it, and held to that property's own constraints as it is typed.
A list is entered one item at a time, items added and removed without retyping the rest, and a list of objects takes a row of fields per item.
A value the catalogue does not describe is entered as JSON.
Fields of a secret value are masked as they are typed.
A known variable that is secret is entered as one.
One the catalogue limits to other scopes is not offered, nor one limited to application types the scope holds none of, a machine with nothing on it being offered every type.

A scope can instead be edited as text, one variable per line as YAML, with names completing from the catalogue as they do in a row.
A value is read as it would be in an inventory file (see [ANSIMP](import.md), "Reading the file").
A secret appears as a placeholder that is kept unless a new value is typed over it, and a value tagged as secret is set as one.
Saving applies only what changed, as variables set and removed, and is refused while a line breaks a rule, naming the line.
The text editor is opened from a button on the scope, or from a keyboard shortcut acting on the scope that holds focus.
