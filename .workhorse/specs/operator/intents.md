---
id: INT
---

# Restore intents

The restore operator advertises four intents to Canopy.
Each names what a replica restored for it is for, the semantics it opts into, and the parameters an operator fills in when declaring a replica (see [RST](../public-server/restore-replicas.md), "Consumer capabilities").
Canopy dispatches only the intents the operator advertises, and applies the behaviour of the semantics it recognises.

## The intents

The four intents are `verify`, `upgrade`, `analytics` and `reporting-schema`.

`verify` restores a snapshot, proves it comes up healthy, and discards the replica.
It takes no parameters.

`upgrade` restores a snapshot, applies a target version's schema migrations to it to prove the upgrade survives the group's own data, and discards the replica.
It takes the starting state its migration runs against: the schemas dropped from the restore before the migration, and SQL run against it first.
The defaults model what a real upgrade of a Tamanu application does before its migrations, dropping the reporting schema whose views block the migration and clearing the server identity and the values encrypted under another key that a copy cannot read.

`analytics` keeps a long-lived read-only query replica restored from the latest snapshot, following new snapshots as they arrive.
It is parametrised by restore cadence, exposure on the tailnet, resource sizing, storage caps, extra users, persistent schemas, a migration target with the starting state its migration runs against, and how long to wait for the replica to become ready.
Persistent schemas are migrated into each restore and kept across restores.
An exposed replica is reachable on the tailnet under a host name derived from its name.
Redaction is an option Canopy controls on a replica, and the parameters that locate the masking manifest and resolve its version are set by Canopy (see [RST](../public-server/restore-replicas.md), "Redaction").

`reporting-schema` restores a snapshot, migrates it to the version Canopy names, builds a Tamanu reporting schema against it, registers the result to Canopy, and discards the replica.
It takes the image that runs the build as a parameter, and the same starting-state parameters as `upgrade`.

## Semantics

Each intent opts into the semantics below, whose behaviour Canopy defines (see [RST](../public-server/restore-replicas.md), "Semantics").

| Intent | Semantics |
| --- | --- |
| `verify` | `check`, `once` |
| `upgrade` | `check`, `once`, `migrate` |
| `analytics` | `check`, `url`, `redact` |
| `reporting-schema` | `check`, `once`, `migrate`, `reporting-schema` |

A replica of an intent carrying `url` reports a link to itself, which for an exposed replica is its address on the tailnet.

## Analytics resource sizing

An analytics replica's Postgres pod is sized from its snapshot unless the operator pins it, and the parameters that pin a resource touch only the resource they name.
Postgres memory is derived from the snapshot size, floored by the intent's memory floor and capped by `resources_maximum`, and is requested and limited at the same value.
The CPU request comes from the intent's floor, a fraction of a core sized for an idle query replica, and no CPU limit is set, so bursts draw on the node's spare CPU.
`cpu_request` and `cpu_limit` set the Postgres CPU request and limit, and leave memory derived from the snapshot.
`memory_request` and `memory_limit` pin Postgres memory in place of the derived value, and setting only one of them pins both request and limit to that value.
A sizing change reaches the replica's running Postgres pod as soon as the replica's configuration carries it, restarting the pod, without waiting for the next restore.

## Migrating a restore to a target version

A restore can apply a target version's schema migrations once its database is healthy and before any switchover.
Two intents set a target, by different routes.

The `upgrade` target is named by Canopy on the worklist entry, through the `migrate` semantic: the version Canopy computes the application should prove it can move to.
The `analytics` target is the `migrate_to` parameter, a Tamanu version the operator chooses for that replica.

`upgrade` reports the migration outcome to Canopy against the version it targeted, as a version-readiness signal.
Backup health and version readiness stay separate signals: a restore whose backup came up healthy but whose migrations failed reports the failing migration as the verdict, and not a failed restore.

The `analytics` migration outcome is not reported to Canopy.
An analytics replica migrates to give the operator an upgraded database to work against, not to verify a version, so it carries no version-readiness signal.

## Building a reporting schema

A reporting schema follows from a Tamanu version's own schema and from the configuration of the group's database together, so it can only be built against a database of that group at that version (see [RPT](../public-server/reporting-schemas.md)).
The operator is where such a database exists.

A `reporting-schema` restore migrates to the version Canopy named, then runs the build against the migrated database, before any switchover.
The build runs as a Job from the image named by the `builder_image` parameter.
The operator does not know how a schema is made: it hands the build a database and takes back what the build emits.
It passes the build the connection to the restored database, the version it migrated to, and the group the snapshot came from.

The build returns the schema to the operator over a callback, since the Job's termination message is too small to carry it.
The operator registers the returned schema with Canopy as an artifact of the exact version, scoped to the group, sending the bytes over the connection it already holds.
It is issued no credential to any store.

The operator reports the build's outcome to Canopy alongside the replica's own health, as it reports a migration outcome: a replica that came up healthy but whose build failed reports a healthy restore and a failed build.
A build that fails does not fail the restore, since the replica was sound and what failed is the schema, which Canopy grades separately.

## Migrated analytics replica

An operator who wants an upgraded database to run read-only processes against sets `migrate_to` on an analytics replica.
The result stays upgraded and queryable, where an `upgrade` replica is a throwaway check.

An analytics replica with `migrate_to` set migrates each restore to that version and keeps the migrated replica running, reachable on the tailnet when the replica is exposed, as any analytics replica is.
It follows new snapshots like any analytics replica, re-migrating each restore to the target version, with restore cadence throttled by the replica's minimum time to live.
An analytics replica with `migrate_to` unset is a plain query replica with no migration step.
A migrated analytics replica draws on the same parameters as any analytics replica for exposure and resource sizing, with no configuration surface of its own.
