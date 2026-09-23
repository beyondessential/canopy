---
id: FLT
---

# Machines, applications, and identities

Canopy's fleet is made of three things: **machines**, the **applications** running on them, and the **identities** that authenticate to Canopy.

Machine-level facts belong to the machine and application-level facts to the application, so a host running two workloads reports its platform, memory and filesystems once rather than once per workload.

## Cardinality

An application runs on exactly one **host**.
A host is a machine, or a Kubernetes cluster where the application is scheduled across one rather than run on a box (see [K8S](../monitoring/kubernetes.md)).
A host of either kind carries any number of applications, including none.

Which kind of host an application has decides what facts it has a place for.
An application on a machine has the machine's platform, memory and filesystems standing behind it; an application on a cluster has no box of its own, so it reports no machine facts and presents none.

A machine has at most one identity, and an identity belongs to at most one machine.
Identities that authenticate something other than a machine — an operator's credential, a relay — belong to no machine at all.

## Machines come from operators

An operator creates a machine and places it in a group.
Canopy issues an enrolment ticket, the agent on the box presents it, and the machine is enrolled.

The group is the only thing an operator supplies, because it is the only thing that exists nowhere else.
Which group a box belongs to is an organisational fact the machine has no way of knowing; what is installed on the box is not.

A machine that has been created but has not yet reported has no applications, and presents as one that has not checked in rather than as an error.

## Applications come from reports

A report is the only thing that creates an application.
An operator never enters an application, its type, or its version, and there is no flow that asks them to.

Canopy adopts what it is told without ceremony: an application it has not seen before on a machine that reports it is created and monitored from that moment.

A report never removes an application.
An application that stops appearing in its machine's reports becomes unreachable and stays, however long it stays away, and only an operator archives it (see [CHK](../monitoring/checks.md), "Reachability").

That asymmetry is deliberate.
Because reports create applications, a reporter that malfunctions and reports nothing would otherwise delete the applications it was responsible for, and monitoring would disappear at the moment it was most needed.
The worst a malfunctioning report can do is make an application read as unreachable, which is loud, rather than absent, which is quiet.

## Naming

A machine is named by the operator who creates it.

An application's name is optional and an operator's alone to set.
An application with no name given presents as the sentence case of its type, so an application of type `tamanu-central` reads as "Tamanu central".

A group containing several applications of one type shows that name repeated, told apart by the machine each runs on and the rank it sits under.

## Archival

A machine and an application are each archived rather than deleted, and each is archived on its own.
Archiving a machine archives the applications on it, a box going away taking its workloads with it.

An archived machine or application leaves the live fleet, and its record and history remain.

## Groups

An application belongs to exactly one group, and so does a machine.

An operator sets the group on the machine, and the applications on that machine take it.

An application's group is never set independently of its machine's, so the two cannot disagree.
Moving a machine to another group moves the applications on it, and there is no separate move for an application that runs on a machine.

An application hosted by a cluster takes its group from where it is deployed rather than from an operator, a cluster carrying applications of many groups at once (see [K8S](../monitoring/kubernetes.md), "The shape Canopy relies on").
A cluster therefore belongs to no group itself, unlike a machine.

### Environments

Rank is an application's, so a group's environment — its members at one rank — is a set of applications.

A machine serves one environment in practice, since a box is not given a production workload and a demo one.
Canopy holds no relationship between the two, because nothing it does turns on one: a machine's stage is derived instead, as the highest rank among the applications on it (see [APP](application-types.md), "Billing attribution").
Deriving it leaves a mixed box well-defined without anyone having to keep a machine's rank in step with what runs on it.

## What each carries

A **machine** carries the name its operator gave it, its identity, its group, where it is (whether it is cloud-hosted and its geolocation), and how long it may be silent before it is considered unreachable.

An **application** carries its type, its rank, its optional name, its public name, the URL it is reached at, the DNS names it serves at and the name-management permissions and pause state that work from those names, its notes and tags, and how long it may be silent before it is considered unreachable.

A DNS name an application serves at is held by that application alone across the fleet, which is what lets a request about a name resolve to one application on a machine hosting several (see [CRT](../public-server/certificates.md), "Declared names").
Its URL is where an operator reaches it, which is a presentation concern rather than an authorisation one (see [SVC](../private-server/service-links.md)).

Both carry tags and effective billing labels, so a check filed against either can be graded by policy rules against the tags of its own target.
An application's type is among the reserved read-only tags on applications, together with the software and role it decomposes into (see [APP](application-types.md)), and appears on no machine, not being a property of a box.

A machine's own name as the operating system reports it is a reported figure rather than a field an operator sets (see [FIG](../private-server/figures.md)).
It is distinct from the DNS names an application serves at: two applications on one machine share one hostname and serve at names of their own.

## Editing

Editing is machine-first: one form per machine, holding the machine's own section and one section per application on it.

So a machine fact has one place it is edited, and a shared box is edited where everything sharing it is visible — a change to the box is visibly a change to every workload on it.
Editing an application leads to that form, its own fields being a section of it rather than a form of their own.

The group is offered on the machine alone, since the applications on it take it and an application's group is never set independently.
An identity is not offered at all: it is bound by enrolment rather than by editing a form.
A machine's name is required when it is created and not when it is edited, so a box that arrived without one stays editable.

## Editing

Editing is machine-first: one form per machine, holding the machine's own section and one section per application on it.

So a machine fact has one place it is edited, and a shared box is edited where everything sharing it is visible — a change to the box is visibly a change to every workload on it.
Editing an application leads to that form, its own fields being a section of it rather than a form of their own.

The group is offered on the machine alone, since the applications on it take it and an application's group is never set independently.
An identity is not offered at all: it is bound by enrolment rather than by editing a form.
A machine's name is required when a box is created and not when it is edited, so a box that arrived without one stays editable.

## Navigating the two grains

The fleet is browsed by group, and its listing offers the groups, the archived records, and the fleet's figures.

Everything the fleet holds is addressed beneath the fleet: its listings, its applications, its machines, and the clusters hosting applications, in the operator UI and in the private API alike.
So a listing and a record are never told apart by which of them a name happens to match first, and an address says which grain it names.
Every address these pages and endpoints answered at before still resolves to where that thing now lives, because a link into Canopy outlives a rename.
Every machine sits in a group and every application takes its machine's, so a group is a complete index of the fleet.

Each grain has a page of its own, and each names the other, so an operator moves between a box and the software on it without a search.

A machine's page presents what the box reports about itself — its platform, its processors, its memory, its clock, how long it has been up — together with the machine's own health, the checks filed against it, and the applications running on it.
A version or a database engine is not there, those being a workload's rather than a box's.

An application's page names the machine it runs on.

A group presents its machines, and under each the applications on it, bucketed by rank.
A machine takes the rank of the highest-ranked application on it, which is the same derivation its billing stage uses (see [APP](application-types.md), "Billing attribution").
A machine carrying nothing yet appears in the group as awaiting check-in rather than being absent, since an operator who has just added a box needs to see it.

## Identities

An identity carries a role naming what it authenticates: a machine, an administrator, a releaser, a backup-restore agent, or a relay.

The machine an identity belongs to is resolved only where it is needed.
A request authenticated as a machine resolves the machine it belongs to; a request authenticated as anything else resolves no machine, so the association is invisible to everything with no business with it.

Because an identity belongs to at most one machine, resolving it is unambiguous.
