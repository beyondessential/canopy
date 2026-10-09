---
id: NAM
---

# Application DNS names

An application reaches Canopy for the two things it cannot do for itself about its own DNS names: publishing the address records that make a DNS name resolve (see [ADR](addresses.md)), and obtaining a TLS certificate for it (see [CRT](certificates.md)).
Both are confined to DNS names within the domains the application's group controls, and both are refused unless an operator has granted that application the matching permission (see [DOM](../servers/domains.md)).

Canopy is the only holder of DNS write access and of the certificate authority account.
An application holds neither, which is the point: a fleet where every application carried zone credentials would put the whole zone at the mercy of its least-defended member.

The two are separate features that share infrastructure, since both end in a write to the zone.
Each has its own declarations, requests, undeclared records, denials, and presentation, so that an application may use either without the other, and an operator can decide about one without deciding about the other.
This spec is what they have in common: the DNS name as the unit both act on, the one application that is allowed to hold it, how a request is resolved to that application, what is recorded when it cannot be, and how both are presented.
Everything that differs by kind is in the spec for that kind.
Throughout, a *kind* is addresses or certificates.

## Declared DNS names

An operator declares the DNS names an application serves, separately for addresses and for certificates, and a request about a DNS name declares it for the application the request resolves to, for the kind the request is (see "Resolving the application").
A declaration is what ties a DNS name to the software that answers on it, and it is what a request is resolved against: an address registration against the DNS names declared for addresses, and a certificate request against the DNS names declared for certificates.
The two kinds of declaration are independent.
Declaring a DNS name for addresses neither requires nor implies declaring it for certificates, and each kind is declared, released, refused, and presented on its own.

A DNS name is held by at most one application across the whole fleet, whichever kinds it is declared for.
An application may hold a DNS name for addresses alone, for certificates alone, or for both, but two applications never hold the same DNS name, one for each kind.
Declaring a DNS name another application already holds, for either kind, is refused, and the refusal names the application holding it, so an operator can see what to release first.
Exclusivity is what makes a DNS name resolve to one application without Canopy having to guess which of a machine's workloads a request is about, and what stops a DNS name pointing at one application while a certificate for it sits with another.

An operator may declare a DNS name that lies outside every domain the application's group controls, since the group may be about to claim the domain it sits under.
Such a declaration routes requests like any other, and every request about it is then refused for want of a domain until the group controls one covering it (see "Identity and authorisation").

An application may declare several DNS names, each held exclusively.
Releasing a DNS name for a kind ends the application's hold on it for that kind and leaves the records and certificates already in place, as revoking a grant does.
Releasing it for the last kind it was held for frees the DNS name to be declared by another application.
What was published stays published and what was issued stays held and collectable until it expires.
What stops is Canopy acting on that DNS name for that kind for that application: after a release for certificates its certificates are no longer renewed and no longer raised as running out, since renewing past a release would order for a DNS name another application may now serve, and reporting a deliberate release as a fault is noise.

## Identity and authorisation

A certificate or address request authenticates as the machine the requesting application runs on, by either transport Canopy already accepts for devices (see [DID](machine-identity.md)).
An identity belongs to a machine rather than to the software on it (see [FLT](../servers/overview.md), "Identities"), so which application a request concerns is resolved from the DNS name it asks about, and from what the request says about itself, rather than from the credential it presents.
Because a DNS name is held by one application, a declared DNS name resolves unambiguously however many applications the machine hosts.

Every request is checked in the same order, and each check is reported distinctly so a misconfiguration is diagnosable from the refusal alone:

1. The caller authenticates as an identity belonging to a live machine.
2. The request resolves to one application on that machine (see "Resolving the application").
3. That application has the grant the request needs — DNS management for addresses, certificate issuance for certificates.
4. The requested DNS name lies at or beneath a domain the application's *own group* controls.
5. A managed zone covers that domain, so Canopy can act on the DNS name at all.

A DNS name within another group's domain is refused as if unclaimed: the refusal says the application's group does not control it, and never that another group does, so the endpoint is not a directory of other groups' DNS names.

### Resolving the application

A request about a DNS name denied to the machine for the kind of the request is refused as denied before it is resolved (see "Denied DNS names"), so a denial holds however the request would otherwise have resolved.

A request starts from every application on the machine and is narrowed in this order, stopping as soon as one application remains:

1. Where an application on the machine holds the requested DNS name, for either kind, to that application.
2. Where the request names an application type, to the applications of that type.
3. To the applications holding the grant the request needs whose group controls a domain covering the DNS name.

A request left with no application, or with several, is refused as undeclared for its kind.

A machine hosting exactly one application resolves to it before anything narrows, unless the request contradicts it (see below), and its requests go on to the checks that follow, so a missing grant or an uncovered domain is refused as that rather than as undeclared.
Naming a type is optional, because an agent on a single-application machine has nothing to tell apart, and an agent that cannot tell which of its workloads serves a DNS name may still be resolved by the grants alone.

A request naming a type the machine contradicts is refused before it is narrowed: one other than the type of the application on the machine holding the DNS name, or one none of the machine's applications is.
The refusal names the holding application's type, or the types the machine's applications are, and is distinguishable from every other refusal, since its remedy is correcting the agent or registering the application rather than waiting.
That is the machine's own business, already in its entitlements, and following the request silently would leave an agent serving a certificate attributed to the workload it said it was not, and declare the DNS name for that workload.

A request that resolves declares its DNS name for the application it resolved to, for the kind the request is, so later requests and renewals resolve from the declaration.
A request for one kind about a DNS name the resolved application holds only for the other kind declares it for this kind as well, the grant check having passed.
A DNS name another application holds cannot be declared that way, and the request is refused as undeclared.

The undeclared refusal is distinguishable from every other refusal.
Its remedy is an operator declaring the DNS name for the kind asked about, or the agent naming the type, so an agent can tell a DNS name waiting on that from one it is not entitled to, and wait rather than report a fault.

The refusal reads the same whether the DNS name is held by an application elsewhere or by nobody, so the endpoint is not a directory of what other machines serve.
A DNS name an application on another machine holds narrows exactly as one nobody holds, so its request meets the same checks in the same order, and is refused as undeclared only where it would otherwise have declared the DNS name.

### Undeclared requests

A request refused as undeclared is recorded against the machine that made it, so an operator learns that a declaration is wanted from Canopy rather than from the agent's alerts.
The record holds the DNS name, whether it was asked about for addresses or for a certificate, and when the machine last asked.
The two kinds are recorded separately, so a machine that asks about one DNS name both ways has two records, each presented with the kind it is for.
A machine asking again about the same DNS name for the same kind updates the one record rather than adding another.
A machine's records are bounded, since the DNS name is the machine's own input: past the bound a new record is refused as usual without being recorded, and the records already held stand.

The record reads the same whether the DNS name is declared by an application on another machine or by nobody.
It is presented to operators alone, who already see the whole fleet, so it tells the asking machine nothing it was not already told.

An operator disposes of an undeclared request by declaring the DNS name for its kind on one of the machine's applications, or by denying that kind to the machine.

A record lasts only as long as it describes something an operator should act on.
It goes when an application on the machine declares the DNS name for that kind, when that kind is denied to the machine for the DNS name, when the machine's next request of that kind about it is accepted, and when a day passes without the machine asking about it that way, so a DNS name the agent has stopped wanting drops off without anyone acting.

### Denied DNS names

An operator can deny a DNS name to a machine for a kind of request, for a DNS name the machine asks about that none of its applications should serve that way.
A denial is of one kind: denying address requests about a DNS name leaves certificate requests about it as they were, and the reverse, and denying both is two denials.
A denial records who made it, when, and an optional note saying why, which is for operators and stays in Canopy.

A request about a DNS name denied to the machine for the kind of the request is refused as denied, naming the DNS name, distinguishably from every other refusal, so an agent can tell a decision against it from a declaration it is waiting on.
Such a request is not recorded as undeclared, so a machine that keeps asking raises nothing however often it asks.

A denial stands until an operator lifts it, or until an operator declares the DNS name for that kind on one of the machine's applications, which is the opposite decision and ends it.
Nothing the machine does lifts a denial, and it outlasts the machine ceasing to ask, since it records a decision rather than an observation.
Lifting a denial and declaring are administrative actions, as denying is.

## What an application may act on

An agent can ask Canopy what DNS names the applications on its machine are entitled to, rather than discovering the boundary by being refused.
The answer is given per application, since the grants and the declared DNS names are each an application's own: for every application on the machine, the domains its group controls, the DNS names it declares for addresses and for certificates, kept apart, which of the two grants it holds, and the DNS names it already has addresses registered or certificates issued for, each with when the certificate expires.

Answering for every application on the machine is what lets one agent serve a box running several: it learns what each of its workloads may do without knowing in advance which of them Canopy holds a grant for.

That is enough for an agent to work ahead of demand: knowing the domains it may use and what it already holds, it can request a certificate before anything asks for one, and renew before expiry, instead of discovering at handshake time that it has nothing to serve.

The answer is available both on its own and on the response to a status push, so an agent that already reports status learns of a new domain or a newly granted permission without asking separately (see [STA](statuses.md)).
Both carry the same content, the standalone form being for an agent that wants it without pushing.

An application with no grants, or whose group controls no domain, is told so plainly: its entry is empty rather than an error, since asking what one may do is not itself a privileged act.
A machine with no applications is answered the same way, with nothing in it.

## Pausing an application

An application can be paused, and while it is, Canopy makes no new changes on its behalf: no certificate is ordered or renewed for it, and no address record of its is changed.

Pausing withdraws nothing already in place.
The records published stand, the certificates held stay held and collectable until they expire, and the group keeps working exactly as it did — a pause being for looking into something, and taking a group off the air not being a neutral act to perform while looking.
What stops is Canopy doing anything *new* on that application's behalf.

Revoking one of an application's certificates pauses that application, without being asked (see [CRT](certificates.md), "Revocation").
Revocation and re-issuance would otherwise chase each other: a key revoked as compromised has its replacement requested within minutes by an agent doing exactly what it was built to do, and if the key leaked because the host was compromised, that replacement hands the same attacker a fresh certificate.
So revoking stops the machinery rather than merely redirecting it, and an operator decides when it is safe to start again.
An operator can also pause an application for any other reason, recording why.

Unpausing is an operator's alone: Canopy never lifts a pause itself, however long it has been in place and however much is expiring under it.
Work resumes where it left off — orders already recorded are worked, renewals fall due again, and address changes waiting to be published are published.

A request from a paused application is refused distinguishably, so an agent can tell being paused apart from being unentitled or misconfigured, and wait rather than hammer.
A pause is not a permission: it says *not now*, where a grant withheld says *not you*.

Because a pause suppresses the alerting that would otherwise chase a certificate running out, the pause itself has to be what is visible.
A paused application presents as paused wherever its DNS names or certificates are presented, with who paused it, when, and why.
And a pause old enough that something has lapsed underneath it is reported against Canopy, since a pause everyone has forgotten is how certificates quietly expire; what wants surfacing is the forgetting rather than the expiry it caused.

## Presentation

DNS names and TLS certificates are presented in separate sections wherever they appear, and each section carries only its own kind.
A DNS names section never shows a certificate or a request for one, and a TLS certificates section never shows an address or a request for one.
An application, a machine, and a group each present the two sections the same way, differing only in which names the section covers and in the detail the spec for the kind gives it, so the two kinds read alike wherever they are met.
A section with nothing to show is absent.

### On an application

An application presents the DNS names it declares for each kind in that kind's section.
An operator declares and releases an application's DNS names for a kind from the same place.
A declared DNS name outside every domain the group controls presents flagged as such, since nothing can be published or certified for it until a covering domain is claimed.

### On a group

A group presents, under each domain it controls, the DNS names declared for each kind beneath it, in that kind's section, with the state the spec for the kind names, so whether a group's DNS names are healthy is answerable without visiting each of its applications.

### On a machine

A machine hosting several applications presents the DNS names declared for a kind together, in that kind's section, since that is where a request of that kind is resolved to one of them.
Each shows the application declaring it.
Any machine with undeclared requests of a kind presents them in that kind's section, each with when it was last asked, with a control to declare the DNS name for that kind on one of the machine's applications and a control to deny that kind of request about it.
A machine hosting one application has them only for a DNS name another application holds, and declaring there is refused with the holder named, which is what an operator needs to release it first.
Declaring from the machine is the same declaration as declaring from the application, refused the same way.
A machine presents the DNS names denied to it for a kind, in that kind's section, each with who denied it, when, and the note, and a control to lift the denial.

### Notices

Undeclared requests are surfaced as a notice rather than as a check: it is shown to whoever reads the pages it appears on and reaches no notification channel, since what it asks for is an operator's decision rather than a response to something down.
A group presents a notice while any of its machines has an undeclared request, saying how many there are, of which kind, and on which machines, and leading to each machine.
The Status page presents one notice across the fleet while any machine has an undeclared request, saying how many there are, of which kind, and in which groups, and leading to each group.
A notice goes once the requests it counts are declared, denied, or drop off.
