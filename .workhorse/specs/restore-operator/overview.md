---
id: PGRO
---

# Restore operator

The restore operator, called pgro, is the first-party restore consumer that runs in a Kubernetes cluster (see [RST](../public-server/restore-replicas.md)).
It restores backups into working Postgres replicas as Canopy directs, reports what it finds, and tears a replica down when Canopy stops asking for it.
It is built and deployed with Canopy, and Canopy is the only thing that drives it.

An operator in these specs is a person, as in the rest of Canopy's specs.
"Restore operator" names the software, which these specs call pgro.

## What drives it

Canopy decides which replicas exist, which snapshot each is restored from, what each is for and how each is configured.
pgro keeps no list of what to restore, and every credential it uses to read a backup repository is issued by Canopy for the run that needs it (see [RST](../public-server/restore-replicas.md), "Credentials").
It reconciles Canopy's worklist against the replicas it is running, creating, refreshing and tearing down replicas until the two match, and reports each replica's outcome back.
pgro needs Canopy's connection details at startup and does not start without them.

## Reaching Canopy

pgro reaches Canopy over the tailnet, through the Tailscale sidecar in its pod, and authenticates as the tailnet node that sidecar runs as.
That node is attached to a Canopy device holding the restore role, as for any restore consumer (see [DTR](../private-server/device-trust.md), "Authentication").
The address of Canopy's tailnet endpoint is part of pgro's configuration, as is the address of the sidecar's proxy that its calls travel through.
The tailnet identity is pgro's only credential to Canopy.
While the sidecar or the tailnet is unavailable, pgro can obtain no worklist or credentials and cannot report, and it resumes when the connection returns.

> [!NOTE]
> The sidecar is in the pod already, to expose analytics replicas on the tailnet.
> Using it as the one path to Canopy means pgro holds no device key to protect and rotate.

## Capabilities

pgro advertises its intents to Canopy at startup and whenever the set changes (see [INT](intents.md)).
Canopy offers those intents to operators when replicas are declared.

## Replicas

Each worklist entry is one replica: a Postgres instance restored from a snapshot and running in pgro's cluster.
Where a replica is scheduled follows the cluster's placement defaults (see [PLACE](placement.md)).
The database roles a replica carries, and how their credentials are exposed, are in [USER](replica-users.md).
What a replica reports, and how Canopy alerts on it, are in [RST](../public-server/restore-replicas.md).
