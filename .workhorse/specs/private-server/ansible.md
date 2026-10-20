---
id: INV
---

# Ansible

Canopy serves Ansible the environment a run acts on: its machines, the applications each carries, and the variables that configure them.
A run reads it as it starts, so it acts on the fleet as Canopy holds it.

## What a run is served

A run is served one environment (see [GRP](../servers/groups.md), "Environments").

Each member is a machine, and carries its identifier, its name, the address it is reached at, the applications on it with their names and types, and its effective variables.
A member names which of those variables are secret.
The environment's own variables are served once beside the members.

Canopy serves the environment's shape and the caller renders it into Ansible's inventory, so no Ansible format appears in the response.

What a run is served carries secret values, so it is served to an administrator (see [ADM](admin-access.md)), and a run reads it as the administrator running it.

## Variables

A variable has a name, and that name is unique per group, per environment, and per machine.
What a run is served merges those three scopes name-wise, a machine's value over its environment's over its group's.

A value is JSON and is served as it was stored.

A name can be prefixed with an application's type, which presents the variable against that application in the operator interface.
The data model and the API scope it no differently.

A variable can be marked as containing a secret, which applies to the whole value rather than to a part of it.
A secret's value is held in Canopy's secret store (see [BKO](backup.md)) rather than beside its name, and is never logged.
Only an administrator can read, set, or replace the value of a secret variable, or remove one.
Non-admins can list secret variables by name only.
Variables must expose whether they are secret over the API; consumers should avoid caching or displaying them if so.
Variables are backed up in the escrow store (see [ESC](../jobs/escrow.md)) to survive the loss of the secret store.

## Reaching a machine

A machine's address is the tailnet name of the device bound to it, or the recorded host of an application on it where no device is bound.
An `ansible_host` variable overrides that.
It names one machine, so it is set at machine scope alone: set wider it would give every machine in the environment one address.
An environment serving two machines the same address is refused, since a run would otherwise configure one box twice and leave the other untouched.

The account a run connects as is the `ansible_user` variable, which is set at any scope.

## Importing an inventory file

An administrator can import an Ansible YAML inventory file into an environment, carrying across the variables it was configured with before Canopy served it to Ansible.
An import is offered only while neither the environment nor any of its machines carries a variable, so it never meets a value already set.
Variables set on the whole group do not hold it back, since an imported environment variable overrides one of the same name rather than clashing with it.

### Reading the file

The file is a tree of groups under `all`, each with optional `vars`, `hosts`, and `children`.
A value is carried as the JSON of what Ansible reads it as, so `yes` and `no` are booleans as they were to a run.
A file that does not parse as an inventory, or has no `all` group, is refused, naming where it fails.
A vault-encrypted value cannot be carried, so a file holding one is refused, naming the variable.

The variables under `all` are set on the environment.
A host's own variables are set on the machine it is matched to.
A variable on a group below `all` is set on each machine matched to a host in that group or beneath it, a host's own value winning over a group's and a nearer group's over a farther one's.
The file's groups carry no further meaning, since a run groups a machine by the applications on it.
`canopy_intent` and `tamanu_clone` are left out, since a run sets them for itself.
Where the file's `tamanu_clone` disagrees with whether the environment is a clone, the preview says so, since the file may have been meant for another environment.

Ansible reaches a host with no `ansible_host` at its key, so where that key is not the address Canopy reaches the machine at, it becomes the machine's `ansible_host`.
A `:port` suffix on the key becomes the machine's `ansible_port` where the host sets none of its own.
A key that is the machine's tailnet name, in full or by its first label, adds nothing.

Every machine a host is imported onto has `managed_by_ansible` set, since the file names it as one a run configures.

The file's comment lines are appended to the group's notes, under a heading naming the environment, since they hold what an operator needs to know before a run, such as where a key is kept or which network to join.

### Matching hosts to machines

Each host is matched against the machines in the environment by the names and addresses it gives: its key, and its `tailscale_name`, `public_hostname`, and `ansible_host`.
Those are compared with each machine's name, the tailnet name of the device bound to it, and the hosts recorded for its applications.
A machine need not be on the tailnet to be matched, and one with no bound device matches by its name and its applications' hosts.
A host that matches exactly one machine is proposed for it, and one that matches none or several is left for the operator to assign.

### Preview

Nothing is applied until the operator has reviewed a preview of the import.
It lists each host beside the machine it is matched to, and every variable that would be set, with its value and where it would be set.
It shows the comments that would be added to the group's notes, which the operator can edit or remove before applying.
The operator can assign any host to a different machine in the environment, or skip it, and a skipped host's variables are not imported.
The import cannot be applied while a host is neither matched nor skipped, or while two hosts are matched to one machine.

A variable is proposed as secret when its name, or the name of any key within its value, contains `salt`, `token`, `secret`, `password`, or `key`.
The operator can mark or unmark any variable as secret before applying.
Secrecy applies to a whole value, so a structured value with one secret field inside it is secret as a whole (see "Variables").

Each variable is held to the same rules as setting one by hand, such as `ansible_host` being set on a machine alone, and the preview names any that breaks them.
An import holding one cannot be applied.

### Applying

An import applies in full or changes nothing.
The environment's presentation shows the imported variables as soon as it is applied.

## Run leases

An environment holds at most one run lease, and a run holds that lease for as long as it runs.
Canopy serves an environment only to the holder of its lease, so two runs never act on one environment at once.

Taking a lease names the environment and what the run intends: to configure the environment as it stands, or to upgrade it.
A lease expires after a fixed period, so a run that dies does not hold an environment shut.
Its holder extends it while the run is still going and releases it when the run ends.

An attempt to take a lease another operator holds is refused, naming who holds it and when it expires.
Taking one over is a deliberate, audited step, so a run never proceeds over another operator's work by accident.

Taking a lease waits only on another take of the same environment. Canopy's own work against the group, its backups and status among it, never holds a run's start up.

### Work under way

Taking a lease is refused while another operator's maintenance window holds over the environment: over its group, or over any of its machines (see [MNT](../monitoring/maintenance.md)).
A window over one machine refuses the whole environment, since a run acts on the environment as a whole.
A window over a machine none of the environment's applications run on refuses nothing.

An operator about to run declares their own window first and is served the environment their window covers.
The window stays over that environment while their lease is held (see [MNT](../monitoring/maintenance.md), "Moving a window").
A target holds at most one open window, so a second operator's declaration amends the first's rather than opening one of their own.
An amendment declares the same work, so the window is the amender's as well as the declarer's: both are served the environment it covers, and everyone else is refused and told whose work is under way.
An amendment a later one replaces stops speaking for the operator who made it.
The refusal lasts exactly as long as the window holds.

### Planned upgrades

A lease taken to upgrade a production environment is refused unless its group has an open upgrade plan (see [UPG](upgrade-plans.md)).
The plan is the permission and its day is not: which night an environment moves is often settled after the plan is recorded.
A lease taken to configure needs no plan, a plan recording a version move and nothing else.

## Refusal

Canopy either serves a run its environment or refuses it.
A refusal must name its reason in such a way that an operator can understand how to resolve it or what/who to wait for.

## Audit

Each environment served to a run is audited, its entry holding the intent its lease declared.
Taking, extending, taking over, and releasing a lease is audited, as is setting or removing a variable (see [AUD](../platform/audit-log.md)).
Importing a file is audited as one entry naming the environment and the variables it set, with the machine each was set on, and whether it added to the group's notes.
A secret's value never appears in a log.

## Presentation

A group presents one of its environments at a time, chosen by the operator where the group has more than one: the machines in it and the variables set at each scope, with a value inherited from a wider scope distinguished from one the machine sets itself.
A secret variable appears by name, with the scope it is set at and when it last changed, and never its value.
Where a lease or a maintenance window holds over the environment, the presentation names it.
The invocation a run is started with is given, filled in with Canopy's address and the environment's identity.
