---
id: ANSIMP
---

# Importing an Ansible inventory file

An administrator can import an Ansible YAML inventory file into an environment, carrying across the variables it was configured with before Canopy served it to Ansible (see [INV](overview.md)).
An import is offered only while neither the environment nor any of its machines carries a variable, so it never meets a value already set.
Variables set on the whole group do not hold it back, since an imported environment variable overrides one of the same name rather than clashing with it.

## Reading the file

The file is a tree of groups under `all`, each with optional `vars`, `hosts`, and `children`.
A value is carried as the JSON of what Ansible reads it as, so `yes` and `no` are booleans as they were to a run.
A value with no JSON form, such as a date, is carried as the text Ansible renders it as.
A file that does not parse as an inventory, or has no `all` group, is refused, naming where it fails.
A vault-encrypted value cannot be carried, so a file holding one is refused, naming the variable.

The variables under `all` are set on the environment.
A host's own variables are set on the machine it is matched to.
A variable on a group below `all` is set on each machine matched to a host in that group or beneath it, a host's own value winning over a group's and a nearer group's over a farther one's.
Where a host sits in groups at the same depth that set the same variable, the value Ansible would have used is carried: the group with the higher `ansible_group_priority`, then the one whose name sorts later.
The file's groups carry no further meaning, since a run groups a machine by the applications on it.
Variables the catalogue marks reserved, such as `canopy_intent` and `tamanu_clone`, are left out, since a run sets them for itself.
Where the file's `tamanu_clone` disagrees with whether the environment is a clone, the preview says so, since the file may have been meant for another environment.

Ansible reaches a host with no `ansible_host` at its key, so where that key is not the address Canopy reaches the machine at, it becomes the machine's `ansible_host`.
A `:port` suffix on the key becomes the machine's `ansible_port` where the host sets none of its own.
A key that is the machine's tailnet name, in full or by its first label, adds nothing.

Every machine a host is imported onto has `managed_by_ansible` set, since the file names it as one a run configures.

The file's comment lines are appended to the group's notes (see [GRP](../../servers/groups.md)), under a heading naming the environment, since they hold what an operator needs to know before a run, such as where a key is kept or which network to join.

## Matching hosts to machines

Each host is matched against the environment's members by the names and addresses it gives: its key, and its `tailscale_name`, `public_hostname`, and `ansible_host`.
Those are compared with each machine's name, the tailnet name of the device bound to it, and the hosts recorded for its applications.
A machine need not be on the tailnet to be matched, and one with no bound device matches by its name and its applications' hosts.
A host that matches exactly one machine is proposed for it, and one that matches none or several is left for the operator to assign.

## Preview

Nothing is applied until the operator has reviewed a preview of the import.
It lists each host beside the machine it is matched to, and every variable that would be set, with its value and where it would be set.
It lists the reserved variables left out, and any group variable an imported environment variable would override.
It shows the comments that would be added to the group's notes, which the operator can edit or remove before applying.
The operator can assign any host to a different member of the environment, or skip it, and a skipped host's variables are not imported.
The import cannot be applied while a host is neither matched nor skipped, or while two hosts are matched to one machine.

A variable the catalogue describes is secret as the catalogue says.
Any other is proposed as secret when its name, or the name of any key within its value, contains `salt`, `token`, `secret`, `password`, or `key`, and the operator can mark or unmark it before applying.
Secrecy applies to a whole value, so a structured value with one secret field inside it is secret as a whole (see [ANSVAR](variables.md), "Variables").

Each variable is held to the same rules as setting one by hand, its catalogue entry among them (see [ANSVAR](variables.md)), and the preview names any that breaks them.
It also names any two members the import would give the same address or host name (see [INV](overview.md), "Reaching a machine").
An import holding either cannot be applied.

## Applying

An import applies in full or changes nothing.
It is checked again against the environment as it stands when applied, and refused if a variable has been set on the environment or its machines, or its members have changed, since the preview.
The environment's presentation shows the imported variables as soon as it is applied.

## Audit

Importing a file is audited as one entry naming the environment and the variables it set, with the machine each was set on, and whether it added to the group's notes (see [AUD](../../platform/audit-log.md)).
A secret's value never appears in a log.
