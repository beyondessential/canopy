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

## History

A variable's history is its entries in the audit log: each value it was set to, its removal, who did each and when (see [AUD](../../platform/audit-log.md)).
A plain value is recorded in the entry that set it, and the value a removal took away in the entry that removed it.
A secret's entries record that it changed, never its value.

An administrator can restore a variable to any earlier value its history holds, which sets it again and is audited as a set.
A removed variable is restored the same way, at the scope it was removed from.

A secret's earlier value, the one a set replaced or a removal took away, is held in the secret store for a week after it was replaced, so a change found wanting after a run can be undone.
Within that week the secret can be restored to it, and after it the value is forgotten.
Restoring a secret holds the value it replaces for a week in turn.

Variables, with the earlier secret values still held, are backed up in the escrow store (see [ESC](../../jobs/escrow.md)) to survive the loss of the secret store.
The escrow is written again within the hour after a secret variable changes, rather than waiting for its cadence, so every value a secret has had reaches it.
Once its week has passed, a secret's earlier value is recovered from the escrow alone.

## Catalogue

The variables the playbooks read are described by a catalogue the infra repository publishes on its main branch, at `ansible/variables.schema.json`.
It is a JSON Schema of an environment's variables, each with its type, its default, what it means, and annotations under `x-canopy`: whether it is secret, the scopes it may be set at, the application types it configures, and whether it is reserved to the plugin.
The same file lists the playbooks a run can start, each with the intent it is run with, the application types it configures, and the tags it takes (see [ANSRUN](runs.md)).
Canopy reads it from the main branch, and keeps using the last copy that read as a catalogue while the repository cannot be reached or publishes one that does not.
Either lasting raises a self-alert (see [SELF](../self-alerts.md)).
A repository that cannot be reached does not replace a verdict that the copy last published does not read: that verdict, and its self-alert, stand until a good read.

A variable the catalogue describes is held to it.
Its value must match its type, it can be set only at the scopes the catalogue allows, and it is secret exactly when the catalogue says so.
A reserved variable cannot be set, since a run sets it for itself.
A name the catalogue does not describe can be set freely, so a variable a playbook has just started reading is not held up by the catalogue.
A value set before the catalogue described it, or before the catalogue changed, stays as it is, secret or plain, and is marked where it no longer matches.
The exception is a value the catalogue comes to call secret, which Canopy moves into the secret store when it reads that catalogue, so a secret is never held beside its name.
A secret the catalogue stops calling secret stays in the secret store until an administrator sets it again.
The application types a variable configures decide where it is offered (see "Presentation"), and are not held to, since a machine is configured before its applications arrive.

## Audit

Setting, restoring, or removing a variable is audited (see [AUD](../../platform/audit-log.md)), with the plain value set or removed (see "History").
Canopy forgetting a secret's earlier value once its week has passed is audited as Canopy's own action.
A change in the catalogue Canopy reads is audited as Canopy's own action, naming the version read, since it changes what a variable is held to, and so is each value it moves into the secret store.
A secret's value never appears in a log.

## Presentation

An environment with variables set presents them collapsed, by count, so they take no room until asked for.
An environment with none, on itself or its machines, offers adding its first beside an import (see [ANSIMP](import.md)).

Expanded, the variables are listed by scope, each nested within the one it narrows: the group's, the environment's indented beneath it, and each of the environment's machines indented beneath that.
Every member is listed, with a switch for whether it is managed by Ansible (see [INV](overview.md), "Managed machines").
A machine that is not managed is greyed, and its variables are collapsed until asked for.
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

Each variable can be edited in its row, its value opening in the same form as adding one, holding the value it has.
A secret's field opens empty, since its value is never shown, and saving it needs a new value.
Saving replaces the value, and leaving the row without saving keeps it as it was.

Removing a variable takes no confirmation, and is followed by an offer to undo it, which restores the value it had.
Each variable opens its history, newest first, offering to restore each earlier value it can be restored to.
Each scope lists the variables removed from it in the last week, each opening its history, so a removal is undone after the offer has gone.
Each scope also opens its own history: every change to a variable set there, removed ones among them, newest first, each naming its variable and offering the same restores.

A scope can instead be edited as text, one variable per line as YAML, with names completing from the catalogue as they do in a row.
A value is read as it would be in an inventory file (see [ANSIMP](import.md), "Reading the file").
A secret appears as a placeholder that is kept unless a new value is typed over it, and a value tagged as secret is set as one.
Saving applies only what changed, as variables set and removed, and is refused while a line breaks a rule, naming the line.
The text editor is opened from a button on the scope, or from a keyboard shortcut acting on the scope that holds focus.
