---
id: RPT
---

# Reporting schemas

A reporting schema is the set of database views a Tamanu application's reports read from.
It is built for one Tamanu version against a database of one environment, since its views follow from that environment's configuration as well as from the version's schema, and every Tamanu application in that environment applies the same one.
Canopy holds zero or one reporting schema per pair of environment and Tamanu version, has one built for every pair it knows of, and offers it to the Tamanu applications of that environment.

## Why it exists

Part of a reporting schema follows from the Tamanu version's database schema and is the same for every deployment on that version.
The rest follows from the configuration of the database it is built from, which only a database carrying that configuration can supply.
A group's environments do not share that configuration: a demo carries sample data a production does not, and a clone diverges from the production it was taken from as soon as either is edited.
A schema for a pair is therefore built from a database of that environment at that version, and ahead of an upgrade no such database exists, since the environment's applications run the version they are leaving.
Canopy restores a group's backups into replicas and migrates them to a version (see [RST](restore-replicas.md)), so it is where such a database is produced, and it knows the version each environment runs and the one it is moving to (see [APP](../servers/application-types.md), [UPG](../private-server/upgrade-plans.md)), so it is where the pairs are known.

## Actors

A **schema builder** produces a reporting schema from a database Canopy has restored and migrated for it, and publishes the result.
It is a restore consumer (see [RST](restore-replicas.md)): a build operates on a replica, so the builder is dispatched, credentialled, and reports over the replica pathways and authorisations, and it advertises an intent carrying `reporting-schema`.
How the builder produces a schema is the builder's own.

An **operator** declares which groups have a builder, marks the declaration that publishes a group's schema, reads which schema each application runs, and asks for the builds the derivation does not produce.

The **device of a machine a Tamanu application runs on** fetches the schema Canopy offers that application and applies it (see [DID](machine-identity.md)).

Canopy owns which pairs exist, the replica a build is given, the artifact that results, and offering it to the group's applications.

## Pairs

A reporting schema is unique per pair of a group's environment and a Tamanu version, and Canopy holds zero or one per pair.
The pairs are, for each environment of a group covered by an enabled declaration marked as publishing its schema, each version a Tamanu application of that environment reports running and the version the environment's open plan moves it to (see [UPG](../private-server/upgrade-plans.md)).
Two environments of one group on one version are two pairs, since the configuration each follows from is its own.
An application Canopy holds no rank for is in none of the group's environments, so it is in no pair until an operator ranks it (see [GRP](../servers/groups.md), "Environments").
That declaration is what covers a group: it names the group, is enabled or disabled, carries the operator's mark, and is audited (see [RST](restore-replicas.md) and [AUD](../platform/audit-log.md)).
Only a published version is in a pair, since a version's migrations reach a builder as its published artifacts (see [ART](../platform/artifacts.md)) and an unpublished one has none.

A pair with no schema is built, and a pair with one is settled.
A settled pair is built again when the version's own artifacts change, since a schema built from a superseded release of the version is not the schema that version describes, and when an operator asks for it.
An operator asking for a pair's build is how a schema is refreshed after the environment's configuration changes.
A rebuilt pair's schema replaces the one it held.
A failed build settles the pair as well, since a build against a fixed version and configuration fails the same way every time, and the pair is built again on the same two events.

## The build contract

Canopy dispatches a build to the builder as a restore replica, through the worklist every replica is dispatched through (see [RST](restore-replicas.md)).
The entry names the group, the environment within it, and the Tamanu version the schema is for, and a machine of that environment running a central Tamanu application, whose snapshot the replica is restored from, since the configuration a schema follows from is held centrally.
The machine is the environment's own central rather than the group's highest-ranked one: restoring one environment's central for every pair would build each environment's schema from another environment's configuration.
It carries what any replica's entry carries: the snapshot to restore, the repo coordinates, and the intent's parameter values.
The replica is migrated to the named version before the build reads it, and is not de-identified, since masking alters the configuration a schema follows from.

The builder obtains read credentials for the restore per run as any consumer does, and no storage credential of any kind for what it publishes (see [RST](restore-replicas.md)).

In the run it reports, the builder registers the **reporting schema** as an artifact of the exact version being built for, scoped to the group and to the environment within it, of type `reporting-schema` on platform `any`, carrying a digest and the bytes themselves, which Canopy holds and serves (see [ART](../platform/artifacts.md)).
The schema is the only type it may register: what is published for an environment is offered to every machine in it and fetched, so the authorisation stays defined with the artifact it was written for.
The builder is authorised to register artifacts for a group whose enabled declaration an operator has marked as publishing its reporting schema, and for no other, and is the one device other than a releaser that registers artifacts (see [ART](../platform/artifacts.md)).
The mark is the operator's alone, and is the whole of the authorisation: a consumer registers the set of semantics it advertises itself, so they shape what Canopy dispatches to it and grant it nothing, and what is published for an environment is offered to every machine in it and applied.
The mark covers the group, and so every environment of it: an environment is not separately marked, since the declaration a build is dispatched for is the group's.
Only a group-wide, non-redacting declaration of an intent carrying `reporting-schema` can carry the mark, which is the same declaration a build is dispatched for, so Canopy asks for no build it would refuse the result of.

A schema is published for the exact version and never for a range, since it follows from the migrations that version applies, and one built against a patch is not the schema another patch of the same minor describes.

## What a build reports

A build reports as its replica's restore report (see [RST](restore-replicas.md)), which names the group, the machine, the snapshot restored, and when it was observed.
Beyond those it carries:

- the **environment** it was built for, which Canopy reads from the machine the replica restored rather than from the report, so the environment a build is filed against and the one its schema is registered against cannot disagree;
- the **version** it was built for;
- the **outcome**, built or failed, and on failure a description of what went wrong;
- a reference to each **artifact** the build registered, of which the schema is one.

The restore's health and the build's outcome stay separate signals from the one report, as a migration test's do: a healthy replica whose build failed reports a healthy restore and a failed build.
Reports are retained indefinitely as an audit trail.

## The offering contract

Canopy offers a Tamanu application's device the schema for the pair of its environment and the version that application reports running, resolved as any group-scoped artifact is (see [ART](../platform/artifacts.md)).
The device's credential resolves to the machine it is enrolled as and so to that machine's group and rank, so it is offered its own environment's schema and can fetch no other's.
A facility application is offered the same schema as the central applications of its environment, since a schema follows the environment and the version rather than the application it was built from.
A machine Canopy holds no rank for is in none of the group's environments, so it is offered no schema until an operator ranks it.

The device compares what its application runs with what it is offered, applies the offered schema where they differ, and reports the result as a check on that application through the status contract (see [STA](statuses.md)), which is graded, presented, and alerted as any source's check is (see [CHK](../monitoring/checks.md)).

## Alerting

A failed build raises a reporting-schema check on the group's central Tamanu application, carrying the failure description (see [CHK](../monitoring/checks.md)).
One check for the group rather than one per environment, since the question it answers is asked of the group, and each pair is an instance of it keyed by the environment and the version so two environments on one version are told apart.
The check is a warning rather than a failure, and does not escalate: the application is up and its reports return rows, and a schema that cannot be built for the version its group is moving to is for whoever maintains the reports rather than whoever is on call.
A replica that failed to restore or come up is the restore's own health rather than a build failure, and is dispatched again as any unhealthy restore is.
The check recovers when the pair is built, and nothing else clears it: an operator asking for the build puts the pair back on the worklist, and the warning stands until a build lands, since asking changes nothing about whether the group's applications can be offered a schema.

Pairs are presented per group and grouped by environment, showing which have a schema, which are being built, and which failed, so whether an environment's applications can be offered the schema for the version they run or are moving to is answered in one place.
An operator can read the SQL a pair published, since what a group's reports will run against is a thing to be able to look at without holding a credential of one of its machines.
