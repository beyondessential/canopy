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
A registered cluster is its relay's identity and a name: Canopy stores no connection credential for a cluster, so it holds no cluster secret to protect or rotate.
Canopy supports several clusters at once, and reads the cluster it runs in itself through a relay like any other.

### Registering a cluster

An operator names the cluster, and Canopy mints its relay's credential and returns the private key once for the operator to install into the cluster, as it does for any provisioned credential (see [DPK](../private-server/provisioned-credentials.md)).
Registering a cluster is therefore what enrols its relay, and an operator reaches both from the one page rather than creating the identity separately.

Canopy confirms the relay is connected and answering before the cluster is registered, so a cluster Canopy cannot read is caught as the operator adds it.
What that confirms is that Canopy can reach the cluster's relay.
A relay that answers while its access to the cluster is still incomplete registers, and reports what it cannot do as checks, so registration turns on the connection rather than on the cluster's permissions being right yet.

### An unfinished registration is kept as a draft

A registration waits on the operator installing the credential and the relay dialling in, so it spans a gap Canopy cannot close on its own.
An unconfirmed registration is kept as a draft, carrying the cluster's name and its relay's identity, and becomes a registered cluster when that relay connects and answers.
The draft is what accounts for the minted identity in the meantime, so an abandoned registration leaves a record of what the identity was for rather than an identity alone.

Only a registered cluster hosts applications, is offered as an application's host, and carries checks.

## A cluster's page

A registered cluster has a page of its own, addressed beneath the fleet beside its machines (see [FLT](../servers/overview.md), "Navigating the two grains").
It presents the cluster's health, its reachability, and the checks filed against it, with the controls each check carries (see [CHK](checks.md), "Operator controls"), and it lists the applications the cluster hosts.
A cluster's checks are read here and on no application, a cluster carrying the applications of many groups (see [CHK](checks.md), "Targets").

The page shows how long the cluster may go unheard before it reads as unreachable, and an admin sets that there.
A cluster's reachability is graded against that threshold like any other target's (see [CHK](checks.md), "Reachability").

The registry links each registered cluster to its page, and an application hosted on a cluster names the cluster on its own page, as one on a box names its machine.
Registering a cluster and re-issuing its credential stay on the registry, which is administration rather than monitoring.

An operator can re-issue a draft's credential, retiring the one before it, for a credential lost before it reached the cluster.
A draft remains until an operator removes it.

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

Under the `kubernetes` source, the relay determines checks about the substrate, what the cluster does with the workloads scheduled on it, and files each at the grain its condition holds for.
The `kubernetes` source is filed only by a relay; no ordinary device reports it and it is reserved from the device API (see [CHK](checks.md), "Sources").
Its checks register already reviewed, each with the policy and documentation its condition warrants.
Its check names are Canopy's own, so each is one catalog entry fleet-wide, whichever cluster or application it is filed against (see [CHK](checks.md), "Names").

The relay determines these checks from the cluster's own objects, read through the cluster's API.
So a check rests on the state of the cluster itself, not on any other monitoring system in the cluster being healthy.

A permission the relay has not been granted makes the check that needed it broken, naming what was refused.
A check the relay cannot read is therefore never absent: absent says the condition does not exist on this cluster, which the relay cannot know.

Per application, the relay determines that the application's workloads can be placed, no pod of it being unschedulable, and that its volumes are bound.

A check under this source can also be scoped past a single application, at either grain.
A check about a namespace targets the group, a namespace being a group at a rank (see [CHK](checks.md), "Targets").
A check about the cluster targets the cluster and is read there, that being the grain such a condition holds for (see [CHK](checks.md), "Targets").

### Checks about the whole cluster

Each core component of a running cluster has a check of its own, detected in the way that component fails.
An ingress controller, a database operator and a node provisioner each fail differently, report their state differently, and are repaired differently, so each is its own condition with its own detail and documentation.
Where one condition holds several times over in a cluster, it is one check with an instance for each, graded and silenced instance by instance (see [CHK](checks.md), "Checks with instances").

Operators reach a cluster's API over the tailnet, through the Tailscale operator's API proxy, and not through the ingress that serves the applications.
So whether operators can get into a cluster is its own check: the proxy is healthy while its workload is ready and the operator reports it connected to the tailnet.
Where the proxy runs inside the Tailscale operator rather than as proxies of its own, the operator reports nothing about its own tailnet connection, so the check reads only that the operator is ready, and its detail says the tailnet half was not read.
A cluster running no API proxy in either form has no such check.
A failed ingress is an outage for an application's users and says nothing about whether an operator can get in to repair it, and neither condition stands in for the other.

Each of a cluster's node pools is an instance of one node pool check.
A pool fails when it is not ready, or when the nodes it launches do not register with the cluster.
Whether a pool's node class can launch nodes is the node class's condition and not the pool's, so a pool is graded on its own conditions apart from that, and one broken class never reads as every pool referencing it failing.

### Workloads broadly not running

One check reports how much of a cluster's workload is running, as the share of desired replicas that are ready, summed across the cluster's Kubernetes Deployments, StatefulSets and DaemonSets and its database clusters.
It is the cluster's own condition, not a rollup of its applications': a large share of everything failing to run has causes no single application's check describes, such as the cluster being out of capacity or unable to provision nodes, and it reports those causes whether or not any other check anticipated them.

The share counts against what each workload asks for, not the pods that exist, so a workload whose pods cannot be created at all counts as not running.
Workloads that are not meant to be running are left out of both sides:

- A workload scaled to zero asks for nothing, whether an operator scaled it or it was put to sleep with its environment.
- A hibernated database cluster still names its instances while it has none running, so it is recognised as hibernated and left out.
- Jobs run to completion rather than keep running, so they are not counted.

So an environment asleep contributes nothing, and putting one to sleep never reads as its workload failing.

The check grades the share in three bands: passed at 90% or above, warning from 80% up to 90%, and failed below 80%.
Crossing back over an edge takes 5 points more than falling across it, so a warning passes again only above 95% and a failure lifts to a warning only above 85%, and a share sitting on an edge holds the result it last had.

A degradation counts sooner the deeper it goes.
The check fails at once below 50%, fails after 2 minutes below 70% or 5 minutes below 80%, and warns after 5 minutes below 90%, each measured from when the share went under that line and stayed under it, and the most urgent that holds is the result.
A shallow dip is what a rolling deploy looks like and passes on its own, while a deep one is not something a deploy does.
Recovery takes effect as soon as the share crosses back over its edge.

The detail carries the share as a number, so an operator regrades the check through a policy rule on it rather than through any threshold setting of the relay's (see [CHK](checks.md), "Policy").

A relay that has just started holds none of the history a band or a hold depends on.
Until it does, it files this check only where the result does not depend on history, which is above 95% and below 50%, and otherwise files nothing for it.
The check keeps the state last filed in the meantime, each filing standing on its own rather than as part of a complete report, so a relay restarting never recovers the check and reopens it minutes later.

## Reachability

A cluster is reachable while its relay is reporting, and its applications are reachable while the relay is reporting them, which is the same rule every other target is held to (see [CHK](checks.md), "Reachability").
So a relay that stops answering makes its cluster and every application on it unreachable, and each recovers as the relay resumes.

Whether an application is serving, as opposed to present, is carried by its own harvested checks rather than by reachability.
