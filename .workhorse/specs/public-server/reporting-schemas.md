---
id: RPT
---

# Reporting schemas

A reporting schema is the set of database views a Tamanu application's reports read from.
It is built for one Tamanu version against a database of one group, since its views follow from the group's configuration as well as from the version's schema, and every Tamanu application in that group applies the same one.
Canopy holds zero or one reporting schema per pair of group and Tamanu version, has one built for every pair it knows of, and offers it to the Tamanu applications of that group.

## Why it exists

Part of a reporting schema follows from the Tamanu version's database schema and is the same for every group on that version.
The rest follows from the group's own configuration, which only a database carrying that configuration can supply.
A schema for a pair is therefore built from a database of that group at that version, and ahead of an upgrade no such database exists, since the group's applications run the version they are leaving.
Canopy restores a group's backups into replicas and migrates them to a version (see [RST](restore-replicas.md)), so it is where such a database is produced, and it knows the version each group runs and the one it is moving to (see [APP](../servers/application-types.md), [UPG](../private-server/upgrade-plans.md)), so it is where the pairs are known.

## Actors

A **schema builder** produces a reporting schema from a database Canopy has restored and migrated for it, and publishes the result.
It is a restore consumer (see [RST](restore-replicas.md)): a build operates on a replica, so the builder is dispatched, credentialled, and reports over the replica pathways and authorisations, and it advertises an intent carrying `reporting-schema`.
How the builder produces a schema is the builder's own.

An **operator** declares which groups have a builder, reads which schema each application runs, and asks for the builds the derivation does not produce.

The **device of a machine a Tamanu application runs on** fetches the schema Canopy offers that application and applies it (see [DID](machine-identity.md)).

Canopy owns which pairs exist, the replica a build is given, the artifact that results, and offering it to the group's applications.

## Pairs

A reporting schema is unique per pair of group and Tamanu version, and Canopy holds zero or one per pair.
The pairs are, for each group covered by an enabled declaration of a `reporting-schema` intent, each version a Tamanu application of the group reports running and the version its open plan moves it to (see [UPG](../private-server/upgrade-plans.md)).
That declaration is what covers a group: it names the group, is enabled or disabled, and is audited (see [RST](restore-replicas.md)).
Only a published version is in a pair, since a version's migrations reach a builder as its published artifacts (see [ART](../platform/artifacts.md)) and an unpublished one has none.

A pair with no schema is built, and a pair with one is settled.
A settled pair is built again when the version's own artifacts change, since a schema built from a superseded release of the version is not the schema that version describes, and when an operator asks for it.
An operator asking for a pair's build is how a schema is refreshed after the group's configuration changes.
A rebuilt pair's schema replaces the one it held.
A failed build settles the pair as well, since a build against a fixed version and configuration fails the same way every time, and the pair is built again on the same two events.

## The build contract

Canopy dispatches a build to the builder as a restore replica, through the worklist every replica is dispatched through (see [RST](restore-replicas.md)).
The entry names the group and the Tamanu version the schema is for, and a machine of the group running a central Tamanu application, whose snapshot the replica is restored from, since the configuration a schema follows from is held centrally.
It carries what any replica's entry carries: the snapshot to restore, the repo coordinates, and the intent's parameter values.
The replica is migrated to the named version before the build reads it, and is not de-identified, since masking alters the configuration a schema follows from.

The builder obtains read credentials for the restore per run as any consumer does, and no storage credential of any kind for what it publishes (see [RST](restore-replicas.md)).

In the run it reports, the builder registers the **reporting schema** as an artifact of the exact version being built for, scoped to the group, of type `reporting-schema` on platform `any`, carrying a digest and the bytes themselves, which Canopy holds and serves (see [ART](../platform/artifacts.md)).
It may register further artifacts beside the schema for the same version and group, under types of its choosing, which Canopy offers as it offers any artifact.
The builder is authorised to register artifacts for a group its enabled `reporting-schema` declaration covers and for no other, and is the one device other than a releaser that registers artifacts (see [ART](../platform/artifacts.md)).

A schema is published for the exact version and never for a range, since it follows from the migrations that version applies, and one built against a patch is not the schema another patch of the same minor describes.

## What a build reports

A build reports as its replica's restore report (see [RST](restore-replicas.md)), which names the group, the machine, the snapshot restored, and when it was observed.
Beyond those it carries:

- the **version** it was built for;
- the **outcome**, built or failed, and on failure a description of what went wrong;
- a reference to each **artifact** the build registered, of which the schema is one.

The restore's health and the build's outcome stay separate signals from the one report, as a migration test's do: a healthy replica whose build failed reports a healthy restore and a failed build.
Reports are retained indefinitely as an audit trail.

## The offering contract

Canopy offers a Tamanu application's device the schema for the pair of its group and the version that application reports running, resolved as any group-scoped artifact is (see [ART](../platform/artifacts.md)).
The device's credential resolves to the machine it is enrolled as and so to that machine's group, so it is offered its own group's schema and can fetch no other's.
A facility application is offered the same schema as the central applications of its group, since a schema follows the group and the version rather than the application it was built from.

The device compares what its application runs with what it is offered, applies the offered schema where they differ, and reports the result as a check on that application through the status contract (see [STA](statuses.md)), which is graded, presented, and alerted as any source's check is (see [CHK](../monitoring/checks.md)).

## Alerting

A failed build raises a reporting-schema check on the group's central Tamanu application, carrying the failure description (see [CHK](../monitoring/checks.md)).
The check is a warning rather than a failure, and does not escalate: the application is up and its reports return rows, and a schema that cannot be built for the version its group is moving to is for whoever maintains the reports rather than whoever is on call.
A replica that failed to restore or come up is the restore's own health rather than a build failure, and is dispatched again as any unhealthy restore is.
The check recovers when the pair is built, and an operator asking for the build is what clears it.

Pairs are presented per group, showing which have a schema, which are being built, and which failed, so whether a group's applications can be offered the schema for the version they run or are moving to is answered in one place.
