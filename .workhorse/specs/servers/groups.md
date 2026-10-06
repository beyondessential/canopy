---
id: GRP
---

# Groups

A group is what Canopy holds shared state against: one backup repository with its passphrase and retention, one notification channel with the delays before an incident on it opens and closes, the domains it claims, and one billing identity.
The fleet is grouped so that what is true of several machines at once is watched, alerted, and paid for in one place.

## Scope

This spec covers the state a group holds, how a group's headline rank is derived, what a group's environments are, and what Canopy calls a group and an environment.

It does not cover which machines and applications a group contains or how an operator puts them there (see [FLT](overview.md), "Groups").

## What a group holds

What makes a group one group is the state Canopy attaches to it rather than anything its members have in common.

Against a group Canopy holds:

- one backup configuration and the repository it names, with the group's passphrase, retention, and placement (see [BKO](../private-server/backup.md));
- one channel its trouble is announced on and the delays before an incident opens and closes, carrying the incidents of every environment in it (see [INC](../monitoring/incidents.md));
- the domains it claims, and the DNS name grants that work from them (see [DOM](domains.md));
- one billing identity, which every member's effective labels derive from (see [APP](application-types.md), "Billing attribution").

A machine belonging to no group carries none of it: its issues reach no incident target and contribute to no incident (see [INC](../monitoring/incidents.md)), and it has no billing attribution (see [APP](application-types.md)).

Membership constrains neither application type nor rank, so a group holds whatever an operator puts in it.

The fleet is organised one group per customer site, holding that site's machines whatever application each runs and whatever environment each serves.
That is how operators use groups rather than a rule Canopy enforces, and a group may equally hold machines belonging to no customer, such as an internal demo.

## Environments

An application's rank is its environment tier: production, clone, demo, test, or dev.
An operator sets it, and once set it can be changed but not cleared.

A machine carries one rank and the applications on it share it, since a box serves one environment, so ranking the machine or any application on it ranks the machine and every application on it (see [FLT](overview.md), "Environments").
An application on a cluster shares its rank with the applications in its namespace in the same way, a namespace being where a cluster application stands for a box (see [K8S](../monitoring/kubernetes.md), "The shape Canopy relies on").
An application arriving on a ranked machine takes the machine's rank, and one arriving in a namespace whose applications carry a rank takes theirs.
One arriving on an unranked machine, or in a namespace where nothing is ranked, is **pending**, and so is the machine or namespace it arrived on, until an operator ranks it.
A machine can be ranked before any application on it has reported, so what arrives on it is never pending.

A group's members at one rank are one of its environments, so a site's production central, the database beside it, and the facilities syncing to it are that site's production environment.
A machine is in the environment its rank names, whether or not anything on it has reported yet.
A pending application or machine is in none of the group's environments, and a group with nothing ranked has no environments until something in it is ranked.

An environment is where a group's applications are going next: it holds at most one open upgrade plan, and the closed plans that preceded it, so a group holds as many open plans as it has environments going somewhere (see [UPG](../private-server/upgrade-plans.md)).
An environment is also what trouble in it is an incident against, announced on its group's channel and delayed by its group's grace, and trouble with what the group holds as a whole, such as its backups, is an incident against its headline environment (see [INC](../monitoring/incidents.md)).
A maintenance window covers one environment where an operator declares it over one (see [MNT](../monitoring/maintenance.md)).
An environment's version is its own central's, derived the way a group's headline version is (see [APP](application-types.md), "Versions").
Everything else Canopy attaches belongs to the group, and it presents a group's members under their rank, its pending ones apart.

## A group's headline rank

A group holds no rank of its own.
Its headline rank is the highest rank held by any of its applications, production outranking clone, then demo, then test, then dev.
The fleet listing buckets each group under its headline rank, and a group with nothing ranked is left out of that bucketing.
A group's billing stage is the same value (see [APP](application-types.md), "Billing attribution").

The headline rank is distinct from the headline version, which is the version the group's highest-ranked central reports (see [APP](application-types.md), "Versions").
A group's headline environment is its members at its headline rank, and a group with nothing ranked has none.

## Naming

Canopy calls a group a group wherever it appears: in the operator interface, in its API, and throughout this spec set.
An environment names a group's members at one rank, which is the unit some infrastructure outside Canopy calls a deployment.
The Canopy instance names an installation of Canopy itself, so a zone list, an ingress, or an account key belongs to the Canopy instance rather than to any group.

The `billing.deployment` cost-allocation label carries the group's name and keeps that spelling, because it is read outside Canopy: by cloud cost allocation, and by every machine reading its effective tags (see [APP](application-types.md), [STA](../public-server/statuses.md)).
