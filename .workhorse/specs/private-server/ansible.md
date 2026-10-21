---
id: INV
---

# Ansible

Canopy serves Ansible the environment a run acts on: its machines, the applications each carries, and the variables that configure them.
A run reads it as it starts, so it acts on the fleet as Canopy holds it.

## What a run is served

A run is served one environment (see [GRP](../servers/groups.md), "Environments").

Each member is a machine, and carries its identifier, its name, its host name (see "Reaching a machine"), the address it is reached at, the applications on it with their names and types, and its effective variables.
A live machine at the environment's rank is a member once a device is bound to it or an application on it has reported, since a run can reach it only then.
One with a bound device and nothing on it yet is a member, since a run is how its applications get there.
A member with nothing on it is served with the application type its run assigns it, if any, so the run configures it as that (see "Machines with nothing on them").
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

### Catalogue

The variables the playbooks read are described by a catalogue the infra repository publishes on its main branch, at `ansible/variables.schema.json`.
It is a JSON Schema of an environment's variables, each with its type, its default, what it means, and annotations under `x-canopy`: whether it is secret, the scopes it may be set at, the application types it configures, and whether it is reserved to the plugin.
The same file lists the playbooks a run can start, each with the intent it is run with, the application types it configures, and the tags it takes.
Canopy reads it from the main branch, and keeps using the last copy that read as a catalogue while the repository cannot be reached or publishes one that does not.
Either lasting raises a self-alert (see [SELF](self-alerts.md)).

A variable the catalogue describes is held to it.
Its value must match its type, it can be set only at the scopes the catalogue allows, and it is secret exactly when the catalogue says so.
A reserved variable cannot be set, since a run sets it for itself.
A name the catalogue does not describe can be set freely, so a variable a playbook has just started reading is not held up by the catalogue.
A value set before the catalogue described it, or before the catalogue changed, stays as it is and is marked where it no longer matches.
The exception is a value the catalogue comes to call secret, which Canopy moves into the secret store when it reads that catalogue, so a secret is never held beside its name.
The application types a variable configures decide where it is offered (see "Variables" under "Presentation"), and are not held to, since a machine is configured before its applications arrive.

## Reaching a machine

A machine's address is the tailnet name of the device bound to it, or the recorded host of an application on it where no device is bound.
An `ansible_host` variable overrides that.
It names one machine, so it is set at machine scope alone: set wider it would give every machine in the environment one address.
An environment serving two machines the same address is refused, since a run would otherwise configure one box twice and leave the other untouched.

A member is served under a host name, which is what a run's limit and output name it by.
It is the first label of its address where that is a tailnet name, and the address itself otherwise.
Where two members would share that name, each is named instead by its Canopy name, reduced to what Ansible accepts in a host name, or by its identifier where nothing of its name remains.
An environment where two members would still share a host name is refused, since Ansible would merge them into one host.

The account a run connects as is the `ansible_user` variable, which is set at any scope.

## Importing an inventory file

An administrator can import an Ansible YAML inventory file into an environment, carrying across the variables it was configured with before Canopy served it to Ansible.
An import is offered only while neither the environment nor any of its machines carries a variable, so it never meets a value already set.
Variables set on the whole group do not hold it back, since an imported environment variable overrides one of the same name rather than clashing with it.

### Reading the file

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

The file's comment lines are appended to the group's notes (see [GRP](../servers/groups.md)), under a heading naming the environment, since they hold what an operator needs to know before a run, such as where a key is kept or which network to join.

### Matching hosts to machines

Each host is matched against the environment's members by the names and addresses it gives: its key, and its `tailscale_name`, `public_hostname`, and `ansible_host`.
Those are compared with each machine's name, the tailnet name of the device bound to it, and the hosts recorded for its applications.
A machine need not be on the tailnet to be matched, and one with no bound device matches by its name and its applications' hosts.
A host that matches exactly one machine is proposed for it, and one that matches none or several is left for the operator to assign.

### Preview

Nothing is applied until the operator has reviewed a preview of the import.
It lists each host beside the machine it is matched to, and every variable that would be set, with its value and where it would be set.
It lists the reserved variables left out, and any group variable an imported environment variable would override.
It shows the comments that would be added to the group's notes, which the operator can edit or remove before applying.
The operator can assign any host to a different member of the environment, or skip it, and a skipped host's variables are not imported.
The import cannot be applied while a host is neither matched nor skipped, or while two hosts are matched to one machine.

A variable the catalogue describes is secret as the catalogue says.
Any other is proposed as secret when its name, or the name of any key within its value, contains `salt`, `token`, `secret`, `password`, or `key`, and the operator can mark or unmark it before applying.
Secrecy applies to a whole value, so a structured value with one secret field inside it is secret as a whole (see "Variables").

Each variable is held to the same rules as setting one by hand, its catalogue entry among them, and the preview names any that breaks them.
It also names any two members the import would give the same address or host name (see "Reaching a machine").
An import holding either cannot be applied.

### Applying

An import applies in full or changes nothing.
It is checked again against the environment as it stands when applied, and refused if a variable has been set on the environment or its machines, or its members have changed, since the preview.
The environment's presentation shows the imported variables as soon as it is applied.

## Run leases

An environment holds at most one run lease, and a run holds that lease for as long as it runs.
Canopy serves an environment only to the holder of its lease, so two runs never act on one environment at once.

Taking a lease names the environment and what the run intends: to configure the environment as it stands, or to upgrade it.
A lease expires after a fixed period, so a run that dies does not hold an environment shut.
Its holder extends it while the run is still going and releases it when the run ends.

### Starting a run

An operator usually starts a run from the operator interface, which takes the lease for the playbook they choose and hands it to the run in the command it gives them.
The lease then waits for the run: it is pending until the run first reads its environment under it.
A lease no run has read within five minutes of being taken is released by Canopy, and the operator is told to start again, so an abandoned start does not hold the environment.

A run can also take a lease of its own when it is started without one, and is then treated as though the lease had been handed to it.

An attempt to take a lease another operator holds is refused, naming who holds it and when it expires.
Taking one over is a deliberate, audited step, so a run never proceeds over another operator's work by accident.

Taking a lease waits only on another take of the same environment. Canopy's own work against the group, its backups and status among it, never holds a run's start up.

### Work under way

Taking a lease is refused while another operator's maintenance window holds over the environment: over its group, or over any of its members (see [MNT](../monitoring/maintenance.md)).
A window over one member refuses the whole environment, since a run acts on the environment as a whole.
A window over a machine that is not a member refuses nothing.

A run's first read under its lease declares a maintenance window over what the run acts on, as the lease's holder: the environment, or the members its limit matches where that is only some of them, noting the playbook it runs, or its intent where a run took its own lease and named no playbook.
A window of the holder's own already covering any of that serves for what it covers, and the run's window covers the rest, so nothing new is declared where the holder's own windows already cover it all.
A window a run declared ends when the lease ends, whether released or expired, and settles as any window does (see [MNT](../monitoring/maintenance.md), "Settling").
Taking over the lease passes the window to the new holder as an amendment, so suspension does not lapse partway through the work.
While it is open, its declarer can hold it, giving it an expected end as a declaration does, and it then no longer ends with the lease but like any other window, when lifted or when that end passes (see [MNT](../monitoring/maintenance.md), "Ending").
The window stays over what it covers while the lease is held (see [MNT](../monitoring/maintenance.md), "Moving a window").
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
A lease taken to start a run records the playbook, its limit and the machines that limit matched, the tags run and skipped, and the application type assigned to each machine with nothing on it.
Further arguments are not recorded, since they are typed freely and can carry a secret.

A run's first read under its lease is audited as the run starting, apart from the routine reads after it, so a reader of the log sees each run without asking for routine entries.
The maintenance window a run's start declares names the lease it was declared for.
Canopy releasing a lease that no run started, and a lease expiring with its run, are audited as Canopy's own actions.
A change in the catalogue Canopy reads is audited as Canopy's own action, naming the version read, since it changes what a variable is held to, and so is each value it moves into the secret store.
Importing a file is audited as one entry naming the environment and the variables it set, with the machine each was set on, and whether it added to the group's notes.
A secret's value never appears in a log.

## Presentation

A group presents one of its environments at a time, chosen by the operator where the group has more than one.

### Runs

An environment offers three ways to start a run: an install, an upgrade, and a run of any playbook the catalogue lists, with the intent it is run with.
The install and the upgrade name their playbooks and intents already.
They are Tamanu's playbooks, so they are offered only where the environment holds a Tamanu application.
The playbooks offered, and the intent each runs with, come from the catalogue.
The run of any playbook is unavailable until Canopy has read a catalogue, and says so.
It offers the playbooks that configure an application type the environment holds, and every playbook where the environment has a member with nothing on it.

A run can be limited to some of the environment's machines.
The limit is picked from the environment's machines and the Ansible groups a run puts them in, or typed as an Ansible host pattern, and the machines it matches are shown as it is typed.
Work cannot be started under a limit that matches no member, or a pattern Canopy cannot evaluate, and the limit says which.
A run can be given tags to run and tags to skip, picked from those the catalogue lists for the playbook.
Any further arguments for the playbook are typed as they would be on the command line.
Tags and further arguments are collapsed until asked for, leaving the limit, which most runs use, the room a list of long host names needs.

Starting work takes the lease, and only then is the command shown, since it carries the lease.
The command brings the operator's checkout of the infra repository up to date with its main branch, by git or by jj as the operator chooses, then runs the playbook against the environment.
Its first step, changing into the checkout, names a place only the operator knows, so it is shown but left out of what is copied.
While the lease waits, the time left for the run to start is shown, and the operator can give it up.
Once the run has started, the presentation says so, and that maintenance is declared.

#### Machines with nothing on them

A run acting on a machine with nothing on it yet assigns the machine the application type it is to carry, chosen from those the playbook configures, or leaves it unassigned.
Where the playbook configures Tamanu and the environment already holds a Tamanu central, each such machine starts assigned a Tamanu facility.
An environment holds at most one Tamanu central, so at most one machine is assigned one, and none where the environment already holds one.
The dialog shows every assignment, and the operator can change any of them before starting work.
The assignments are held with the lease and last for that run, since a machine is grouped by its applications once they report.

A refusal to take the lease is shown before any command, naming its reason (see "Refusal").
Where a lease or a maintenance window holds over the environment, the presentation names who holds it and what they are running, by playbook where the lease names one and by intent otherwise, and offers taking the lease over.

### Variables

An environment with variables set presents them collapsed, by count, so they take no room until asked for.
An environment with none, on itself or its machines, offers adding its first beside an import (see "Importing an inventory file").

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
A value is read as it would be in an inventory file (see "Reading the file").
A secret appears as a placeholder that is kept unless a new value is typed over it, and a value tagged as secret is set as one.
Saving applies only what changed, as variables set and removed, and is refused while a line breaks a rule, naming the line.
The text editor is opened from a button on the scope, or from a keyboard shortcut acting on the scope that holds focus.
