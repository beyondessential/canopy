---
id: K8S
---

# Kubernetes monitoring

Canopy monitors the Tamanu applications running on Kubernetes through one relay in each cluster, which determines their checks and files them, rather than through an agent on each box reporting its own (see [STA](../public-server/statuses.md)).
An application on a cluster is an ordinary application in the fleet, carrying the same check state, health, incidents, and operator controls as any other (see [CHK](checks.md)), monitored through its cluster rather than by an agent of its own.

## The shape Canopy relies on

A namespace holds one environment: a group's applications at one rank, so the Nauru group at the demo rank is one namespace, and separate ranks are separate namespaces (see [GRP](../servers/groups.md), "Environments").
Within a namespace each central and each facility has its own Postgres instance and its own workloads per duty, with no database or workload shared between duties or between applications.
So a namespace's contents map onto applications one for one, each with its own set of workloads and its own database.

## A cluster is a host

A cluster hosts the applications scheduled across it, standing where a machine stands for an application installed on a box (see [FLT](../servers/overview.md), "Cardinality").
An application on a cluster has no machine, so it reports and presents no machine facts, and the checks about what it runs on are the cluster's rather than a box's.

One cluster carries applications of many groups at once, so a cluster belongs to no group itself.
An application on a cluster takes its group from the namespace it is deployed in, a namespace being one group at one rank.

Canopy learns a cluster's applications from what its relay reports, as it learns every application (see [FLT](../servers/overview.md), "Applications come from reports").
The relay names each by a key of its own choosing which is stable across its reports, and Canopy correlates the application by its cluster, that key and its type, exactly as it correlates an application reported on a machine (see [STA](../public-server/statuses.md), "Identifying an application").
So an operator never enters a cluster's applications, and there is no picker to keep in step with what is deployed.

A namespace that goes away takes its applications' reports with it, so they become unreachable and remain until an operator archives them, which is what happens to any application that stops being reported.

## The relay in each cluster

Canopy reads a cluster through a relay Canopy runs inside that cluster, rather than by reaching the cluster's Kubernetes API itself.
The relay holds the permissions for its cluster, and it opens its connection to Canopy outward, so Canopy holds no credential to the cluster and the cluster accepts no connection from Canopy.
Canopy's authority over a cluster is therefore the set of requests its relay answers, each scoped to what a check or an operator action needs, rather than a set of permissions over the cluster's objects.

A relay's connection is continuous, so Canopy observes the loss of a relay directly rather than inferring it from a request failing.
A relay is enrolled as an identity carrying the relay role and belongs to no machine, so it is created, authenticated, tracked, and revoked as any other identity is (see [DTR](../private-server/device-trust.md)).

### What crosses the connection

The relay determines the checks of the cluster and of the applications on it, under both sources, and files the results upward.
Everything crossing the connection is a filed check, an answer to one named question, or an action on an environment, so the cluster's objects stay in the cluster and what Canopy learns of a cluster is what its relay has already made a check of.

The relay holds the current state of what it watches and files a check when that check's result changes, so a condition surfaces when it arises rather than on the next turn of a polling loop.
It also refiles what it holds periodically, so a check's state is re-established after an observation the relay missed, a relay restart, or a reconnection, rather than resting on a change it may never see.

Beyond filings, Canopy asks a relay only for what is not a check: whether the relay is connected and answering, for cluster registration, and the version of the check suite it runs (see [SELF](../private-server/self-alerts.md)).
Each is a named answer to one question rather than a means of reading the cluster.

### Keeping a relay current

Canopy names the version of the check suite each relay should be running, and a relay updates itself to the version named for it.
So the fleet of relays follows Canopy's account of where they should be rather than being rolled cluster by cluster, which is what keeps a change to a check reaching every cluster it applies to.

What Canopy names is a version and never the code itself: a relay obtains its code from where its versions are published, so the most Canopy can ask of a relay is which published version to run.
The cluster carries out the update, so a version that will not start leaves the relay already serving in place and Canopy can name an earlier one to recover from a bad release.
A relay refuses a version below the floor it carries, so it cannot be sent back to a release already known to be bad.
The floor is the relay's own rather than something Canopy supplies, because a floor Canopy could set is a floor Canopy could lower.

### Putting an environment to sleep

Canopy can put an environment to sleep and wake it again.
An environment is a namespace, so the action covers every application in it together and there is no sleeping one application within a namespace.

An environment that has no scheduled expiry cannot be put to sleep, and its relay is what refuses the request, so the restriction holds where the expiry is known rather than resting on Canopy asking correctly.
Canopy gates the action by what the environment is, as it gates any action against production.
Sleeping and waking are available to admins and are audited.

Whether an environment is asleep is a fact Canopy presents on the environment rather than a check, an environment asleep on purpose being nothing to grade.
Each of its applications already carries it as the reason that application's checks are skipped (see "Checks that cannot run there").

## Cluster registry

Clusters are registered in Canopy through a settings page and managed in-app, not through environment variables the process reads at startup.
Registering a cluster enrols its relay, and Canopy confirms the relay is connected and answering before the cluster is saved, so a cluster Canopy cannot read is caught as the operator adds it.
A registered cluster is its relay's identity and a name: Canopy stores no connection credential for a cluster, so it holds no cluster secret to protect or rotate.
Canopy supports several clusters at once, and reads the cluster it runs in itself through a relay like any other.

## What each source reports

A cluster's checks and its applications' checks come from two sources, divided by what each check asserts something about rather than by how Canopy comes to observe it.

The `alertd` source's subject is one application: the thing that has a database, a version, an API, sync state, and a set of duties that ought to be running.
That subject is a coherent assemblage of processes or containers however they happen to be run, so it is the same subject on a cluster as on a box running the Tamanu services directly, and a condition about it is the same check on both.

The `kubernetes` source's subject is the substrate: what the cluster does with those workloads, at grains other than one application.
Coarser than an application, such as a namespace or the cluster itself, or finer, such as a single pod that cannot be scheduled or a volume that will not bind.

A check belongs to a source by its subject, not by whether it has a counterpart on other hosts.
So a check that asserts something about one application and is only expressible in Kubernetes is an application check, reported under `alertd`, and a condition that touches a whole cluster is reported at that grain rather than against the applications that happen to run there.

## Checks harvested for the application

Under the `alertd` source, Canopy files a cluster application's checks by running the same Tamanu check suite that Tamanu applications run elsewhere, against that application, and filing the results itself.
The suite covers the conditions an application derives from its own database (the sync system, FHIR processing, migrations, and the rest), whether its duties are running and on the version they should be, whether its API answers, how much storage headroom it has, and its HTTP error rate.

Because it is the same check implementation, an application on a cluster and one that pushes its own reports share one catalog entry and one policy per check, are graded identically, and cannot drift into subtly different checks (see [CHK](checks.md), "Policy").
A check's thresholds come from that implementation on either host, so there is no per-application threshold configuration to hold and no way for one host's thresholds to drift from another's; operators grade a check through its policy instead.

The relay runs the harvest inside the cluster and reports the results it produces.
It obtains each application's database credentials from the cluster rather than from configuration Canopy holds: the namespace holds the databases and the secret backing each, and the relay reads them there.
So a database credential and the queries the checks run against it stay within the cluster, and what crosses to Canopy is check results.

### Checks that cannot run there

A check that has no meaning for an application on a cluster is absent from what the relay files, rather than reported as anything.
A check is skipped where it applies to the application but could not be read on this pass, which is a different thing an operator reads differently: absent says the condition does not exist here, skipped says it exists and is currently unknown.

An environment scaled to zero with its database hibernated is deliberately asleep rather than in trouble, so its applications' checks are skipped for as long as it stays that way.
A hibernated namespace is still present and its relay still reports, so its applications stay reachable (see "Reachability").

### The harvest reports on the application, never on the harvester

What the harvest files describes the application it names and nothing else.
The detail it carries omits anything that describes the process which produced it: the harvester's host, operating system, uptime, network, and its own version are not the application's, and presenting one as the other would state something false about the application rather than leave a gap.

So a figure an application on a cluster has no source for is simply not reported, and the rules for a figure nothing reports apply as they do anywhere (see [FIG](../private-server/figures.md)).
In particular such an application presents no bestool version, having no such agent installed for it, and the version of the check suite the harvest runs is a property of the relay rather than of any application it serves (see [SELF](../private-server/self-alerts.md)).

## Checks determined about the substrate

Under the `kubernetes` source, the relay determines checks about what the cluster does with an application's workloads and files them.
The `kubernetes` source is filed only by a relay; no ordinary device reports it and it is reserved from the device API (see [CHK](checks.md), "Sources").
Its checks register already reviewed, each with the policy its condition warrants.

Per application, the relay determines that the application's workloads can be placed, no pod of it being unschedulable, and that its volumes are bound.

A check under this source can also be scoped past a single application, at either grain.
A check about a namespace targets the group, a namespace being a group at a rank (see [CHK](checks.md), "Targets").
A check about the cluster targets the cluster, which every application on it presents as its host's, the way an application on a box presents its machine's (see [CHK](checks.md), "A host's checks present on its applications").

## Reachability

A cluster is reachable while its relay is reporting, and its applications are reachable while the relay is reporting them, which is the same rule every other target is held to (see [CHK](checks.md), "Reachability").
So a relay that stops answering makes its cluster and every application on it unreachable, and each recovers as the relay resumes.

Whether an application is serving, as opposed to present, is carried by its own harvested checks rather than by reachability.
