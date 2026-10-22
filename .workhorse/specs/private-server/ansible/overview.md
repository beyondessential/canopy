---
id: INV
---

# Ansible

Canopy serves Ansible the environment a run acts on: its machines, the applications each carries, and the variables that configure them.
A run reads it as it starts, so it acts on the fleet as Canopy holds it.

The variables and the catalogue describing them are in [ANSVAR](variables.md), importing an old inventory file is in [ANSIMP](import.md), and leases and starting a run are in [ANSRUN](runs.md).

## What a run is served

A run is served one environment (see [GRP](../../servers/groups.md), "Environments").

Each member is a machine, and carries its identifier, its name, its host name (see "Reaching a machine"), the address it is reached at, the applications on it with their names and types, and its effective variables.
A live machine at the environment's rank is a member once a device is bound to it or an application on it has reported, since a run can reach it only then.
One with a bound device and nothing on it yet is a member, since a run is how its applications get there.
A member with nothing on it is served with the application type its run assigns it, if any, so the run configures it as that (see [ANSRUN](runs.md), "Machines with nothing on them").
A member names which of those variables are secret.
The environment's own variables are served once beside the members.

Canopy serves the environment's shape and the caller renders it into Ansible's inventory, so no Ansible format appears in the response.

What a run is served carries secret values, so it is served to an administrator (see [ADM](../admin-access.md)), and a run reads it as the administrator running it.
It is served only to the holder of the environment's lease (see [ANSRUN](runs.md), "Run leases").

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

## Refusal

Canopy either serves a run its environment or refuses it.
A refusal must name its reason in such a way that an operator can understand how to resolve it or what/who to wait for.

## Audit

Each environment served to a run is audited, its entry holding the intent its lease declared (see [AUD](../../platform/audit-log.md)).
A secret's value never appears in a log.

## Presentation

A group presents one of its environments at a time, chosen by the operator where the group has more than one, with the ways to start a run on it (see [ANSRUN](runs.md), "Presentation") and its variables (see [ANSVAR](variables.md), "Presentation").
