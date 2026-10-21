---
id: ANSRUN
---

# Ansible runs

How an Ansible run gets to act on an environment: the lease it holds while it runs, the maintenance it declares, and how an operator starts one (see [INV](overview.md)).

## Run leases

An environment holds at most one run lease, and a run holds that lease for as long as it runs.
Canopy serves an environment only to the holder of its lease, so two runs never act on one environment at once.

Taking a lease names the environment and what the run intends: to configure the environment as it stands, or to upgrade it.
A lease expires after a fixed period, so a run that dies does not hold an environment shut.
Its holder extends it while the run is still going and releases it when the run ends.

### Starting a run

An operator usually starts a run from the operator interface, which takes the lease for the playbook they choose and hands it to the run in the command it gives them.
The lease then waits for the run: it is pending until the run first reads its environment under it.
A lease no run has read within a short fixed period of being taken is released by Canopy, and the operator is told to start again, so an abandoned start does not hold the environment.

A run can also take a lease of its own when it is started without one, and is then treated as though the lease had been handed to it.

An attempt to take a lease another operator holds is refused, naming who holds it and when it expires.
Taking one over is a deliberate, audited step, so a run never proceeds over another operator's work by accident.

Taking a lease waits only on another take of the same environment. Canopy's own work against the group, its backups and status among it, never holds a run's start up.

### Work under way

Taking a lease is refused while another operator's maintenance window holds over the environment: over its group, or over any of its members (see [MNT](../../monitoring/maintenance.md)).
A window over one member refuses the whole environment, since a run acts on the environment as a whole.
A window over a machine that is not a member refuses nothing.

A run's first read under its lease declares a maintenance window over what the run acts on, as the lease's holder: the environment, or the members its limit matches where that is only some of them, noting the playbook it runs, or its intent where a run took its own lease and named no playbook.
A window of the holder's own already covering any of that serves for what it covers, and the run's window covers the rest, so nothing new is declared where the holder's own windows already cover it all.
A window a run declared ends when the lease ends, whether released or expired, and settles as any window does (see [MNT](../../monitoring/maintenance.md), "Settling").
Taking over the lease passes the window to the new holder as an amendment, so suspension does not lapse partway through the work.
While it is open, its declarer can hold it, giving it an expected end as a declaration does, and it then no longer ends with the lease but like any other window, when lifted or when that end passes (see [MNT](../../monitoring/maintenance.md), "Ending").
The window stays over what it covers while the lease is held (see [MNT](../../monitoring/maintenance.md), "Moving a window").
A target holds at most one open window, so a second operator's declaration amends the first's rather than opening one of their own.
An amendment declares the same work, so the window is the amender's as well as the declarer's: both are served the environment it covers, and everyone else is refused and told whose work is under way.
An amendment a later one replaces stops speaking for the operator who made it.
The refusal lasts exactly as long as the window holds.

### Planned upgrades

A lease taken to upgrade a production environment is refused unless its group has an open upgrade plan (see [UPG](../upgrade-plans.md)).
The plan is the permission and its day is not: which night an environment moves is often settled after the plan is recorded.
A lease taken to configure needs no plan, a plan recording a version move and nothing else.

## Audit

Taking, extending, taking over, and releasing a lease is audited (see [AUD](../../platform/audit-log.md)).
A lease taken to start a run records the playbook, its limit and the machines that limit matched, the tags run and skipped, and the application type assigned to each machine with nothing on it.
Further arguments are not recorded, since they are typed freely and can carry a secret.

A run's first read under its lease is audited as the run starting, apart from the routine reads after it, so a reader of the log sees each run without asking for routine entries.
The maintenance window a run's start declares names the lease it was declared for.
Canopy releasing a lease that no run started, and a lease expiring with its run, are audited as Canopy's own actions.

## Presentation

An environment offers three ways to start a run: an install, an upgrade, and a run of any playbook the catalogue lists, with the intent it is run with.
The install and the upgrade name their playbooks and intents already.
They are Tamanu's playbooks, so they are offered only where the environment holds a Tamanu application.
The playbooks offered, and the intent each runs with, come from the catalogue (see [ANSVAR](variables.md), "Catalogue").
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

A refusal to take the lease is shown before any command, naming its reason (see [INV](overview.md), "Refusal").
Where a lease or a maintenance window holds over the environment, the presentation names who holds it and what they are running, by playbook where the lease names one and by intent otherwise, and offers taking the lease over.

### Machines with nothing on them

A run acting on a machine with nothing on it yet assigns the machine the application type it is to carry, chosen from those the playbook configures, or leaves it unassigned.
Where the playbook configures Tamanu and the environment already holds a Tamanu central, each such machine starts assigned a Tamanu facility.
An environment holds at most one Tamanu central, so at most one machine is assigned one, and none where the environment already holds one.
The dialog shows every assignment, and the operator can change any of them before starting work.
The assignments are held with the lease and last for that run, since a machine is grouped by its applications once they report.
