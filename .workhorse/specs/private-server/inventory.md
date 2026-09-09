---
id: INV
---

# Environment inventory

Canopy serves an environment's inventory to Ansible: the machines a run acts on, the applications each carries, and the variables that configure them.
A run reads it as it starts, so it acts on the fleet as Canopy holds it.

## What an inventory contains

An inventory covers one environment (see [GRP](../servers/groups.md), "Environments").

Each member is a machine, and carries its identifier, its name, the address it is reached at, the applications on it with their names and types, and its effective variables.
A member names which of those variables are secret.
The environment's own variables are served once beside the members.

Canopy serves the environment's shape and the caller renders it, so no configuration-management tool's own inventory format appears in the response.

An inventory carries secret values, so it is served to an administrator (see [ADM](admin-access.md)), and a run reads it as the administrator running it.

## Inventory variables

A variable has a name, and that name is unique per group, per environment, and per machine.
An inventory's view merges those three scopes name-wise, a machine's value over its environment's over its group's.

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

## Run leases

An environment holds at most one run lease, and a run holds that lease for as long as it runs.
Canopy serves an inventory only to the holder of the environment's lease, so two runs never act on one environment at once.

Taking a lease names the environment and what the run intends: to configure the environment as it stands, or to upgrade it.
A lease expires after a fixed period, so a run that dies does not hold an environment shut.
Its holder extends it while the run is still going and releases it when the run ends.

An attempt to take a lease another operator holds is refused, naming who holds it and when it expires.
Taking one over is a deliberate, audited step, so a run never proceeds over another operator's work by accident.

### Work under way

Taking a lease is refused while another operator's maintenance window holds over the environment: over its group, or over any of its machines (see [MNT](../monitoring/maintenance.md)).
A window over one machine refuses the whole environment, since a run acts on the environment as a whole.
A window over a machine none of the environment's applications run on refuses nothing.

An operator about to run declares their own window first and is served the environment their window covers.
A target holds at most one open window, so a second operator's declaration amends the first's rather than opening one of their own.
An amendment declares the same work, so the window is the amender's as well as the declarer's: both are served the environment it covers, and everyone else is refused and told whose work is under way.
An amendment a later one replaces stops speaking for the operator who made it.
The refusal lasts exactly as long as the window holds.

### Planned upgrades

A lease taken to upgrade a production environment is refused unless its group has an open upgrade plan (see [UPG](upgrade-plans.md)).
The plan is the permission and its day is not: which night an environment moves is often settled after the plan is recorded.
A lease taken to configure needs no plan, a plan recording a version move and nothing else.

## Refusal

Canopy either serves an inventory or refuses it.
A refusal must name its reason in such a way that an operator can understand how to resolve it or what/who to wait for.

## Audit

Each inventory read is logged with the identity that asked for it and the intent its lease declared.
Taking, extending, taking over, and releasing a lease is audited, as is setting or removing a variable.
A secret's value never appears in a log.

## Presentation

A group presents each of its environments: the machines in it and the variables set at each scope, with a value inherited from a wider scope distinguished from one the machine sets itself.
A secret variable appears by name, with the scope it is set at and when it last changed, and never its value.
Where a lease or a maintenance window holds over the environment, the presentation names it.
The invocation a run is started with is given, filled in with Canopy's address and the environment's identity.
