---
id: CRT
---

# Application DNS names and certificates

An application reaches Canopy for the two things it cannot do for itself about its own DNS names: publishing the address records that make a DNS name resolve, and obtaining a TLS certificate for it.
Both are confined to DNS names within the domains the application's group controls, and both are refused unless an operator has granted that application the matching permission (see [DOM](../servers/domains.md)).

Canopy is the only holder of DNS write access and of the certificate authority account.
An application holds neither, which is the point: a fleet where every application carried zone credentials would put the whole zone at the mercy of its least-defended member.

The two are separate features that share infrastructure, since both end in a write to the zone.
Each has its own declarations, requests, undeclared records, denials, and presentation, so that an application may use either without the other, and an operator can decide about one without deciding about the other.
What they have in common is the domain a DNS name must lie within and the one application that is allowed to hold it.

## Why Canopy issues

An application's DNS name resolves to an address that may not be reachable from the public internet, and often is not: a facility application sits behind someone else's NAT.
So the challenge types that prove control by answering on the DNS name's own address are unavailable, and proving control through DNS is the only route left.
Proving control through DNS means writing to the zone, and Canopy already holds that access on the group's behalf.

Centralising issuance also puts the authority's rate limits, the record of every certificate's expiry, and the alerting when renewal stops working in one place — the same place that already watches the fleet.

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

Revoking one of an application's certificates pauses that application, without being asked.
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

## Addresses

An application registers the DNS name it should be reachable at together with the external addresses it is reachable at, and Canopy publishes the address records: the IPv4 addresses as A records, the IPv6 addresses as AAAA records, at that DNS name, in the managed zone it resolves to.

Registering replaces the addresses previously registered for the DNS name, so an application announces a change of address by registering again, and a registration naming no addresses withdraws the DNS name.
Canopy publishes what it is told: it does not verify that an address is really the application's, the grant being the trust boundary rather than any proof of possession.

Canopy changes only records it created itself.
Because zones are shared, a DNS name may be served by records Canopy knows nothing about, and Canopy neither rewrites nor removes those; it records what it has published so it can tell its own records from everyone else's.

A DNS name's addresses are the addresses of the one application that holds it for addresses, so two applications cannot fight over where one DNS name points.

## Certificates

### Requesting

An application generates its own key pair and asks Canopy to certify it, submitting a certificate signing request for a single DNS name.
The private key never leaves the application and Canopy never asks for it: Canopy's part is to prove control of the DNS name and return the signed chain.

The signing request is honoured only for exactly the DNS name requested.
Canopy certifies that one DNS name and no other: a request whose subject or alternative names carry anything beyond the requested DNS name is refused rather than trimmed, because silently issuing something narrower than asked would leave an application serving a certificate it does not expect, and issuing something wider would let one application smuggle another group's DNS name past the authorisation check.
Wildcards are refused: a certificate valid for every DNS name in a group is not something one member should be able to mint.

The key the request certifies must be strong enough to be worth certifying, and Canopy states what it accepts rather than deferring to whatever the authority happens to allow that year.

### Fulfilment is not immediate

Proving control through DNS takes as long as it takes for the authority to see a record Canopy has just published — tens of seconds at best, minutes when a resolver holds a negative answer.
That is far longer than any client will wait mid-handshake, so requesting a certificate and collecting it are separate steps: a request is accepted and acknowledged, Canopy works the order in the background, and the application collects the result when it is ready.

An application therefore holds a certificate before it needs one, rather than obtaining one at the moment a client arrives.
Canopy's contract is only that a request is durable once accepted and that its outcome becomes collectable; scheduling requests early enough to be useful is the application's business.

Repeating a request for a DNS name Canopy already holds a valid certificate for returns the one it holds rather than ordering another, so an application that has lost its local copy — restarted, redeployed, cache cleared — is served without spending the authority's budget.
A request naming a key different from the one already certified is a new order, since the stored chain certifies a key the application no longer holds.

### What Canopy keeps

Canopy keeps the certificate it obtained, the DNS name it covers, the application it was issued for, and when it expires.
It keeps no private key, having never held one.

Holding the chain is what lets Canopy answer a repeat request without a fresh order, renew before expiry without being asked, and report a certificate that is running out.

### Lifetime

An authority may offer certificates of more than one lifetime, named as profiles, and an application's certificates are requested under one of them.
The profiles on offer are whatever the authority advertises, so Canopy presents that set rather than a list of its own, and a profile the authority has withdrawn is reported as unavailable instead of being requested and refused.

An application's profile is an operator's choice per application, because lifetime is a property of how an application is run rather than of Canopy: a cloud-hosted one whose issuance is exercised constantly can carry a short lifetime, where an on-premises one that may be offline for days cannot.
Every application takes the longest profile the authority offers until an operator says otherwise, so a short lifetime is something adopted deliberately for an application rather than a default anyone inherits.

### Renewal

Canopy renews a certificate it holds before it expires, without being asked, and the renewed chain becomes collectable the same way the first one did.
An application that collects periodically therefore stays current without tracking expiry itself, and one that asks again gets whatever is newest.

When to renew comes from the authority when it will say: an authority that publishes renewal information is asked when it would like this certificate replaced, and Canopy renews in the window it names.
Failing that, Canopy renews after a fixed fraction of the certificate's own life has passed.
Neither is a fixed interval, because a fixed interval cannot serve both lifetimes: a window measured in weeks would leave a certificate that lives days permanently overdue, and one measured in hours would renew a long-lived certificate hundreds of times over.
Where the authority accounts for a renewal as replacing a particular certificate, Canopy tells it which, so a renewal is not mistaken for an additional certificate.

Renewal stops when the certificate is no longer wanted: a DNS name whose group has released the domain it sits under is not renewed, nor is a certificate for a DNS name its application no longer declares for certificates, or for an application whose grant has been revoked or that has been archived.
A grant revoked does not withdraw the certificate already issued — it cannot be recalled once it exists — but it does end the renewals that would extend it.

### Revocation

An operator can revoke a certificate Canopy holds, saying why.
Canopy holds the account that obtained it, which is authority enough to revoke it — the application's private key is not needed and is not asked for.

Revocation exists for the day something has gone wrong, so it is reachable where the certificate is presented rather than filed away as a maintenance procedure, and it is destructive enough to confirm before it happens.
It cannot be undone: a revoked certificate stays revoked, and the remedy is a new one.

Canopy stops renewing a revoked certificate and records who revoked it, when, and the reason given.

An application collecting a certificate it holds locally is told that it has been revoked, so it stops serving something clients will reject, and is told separately whether the key it holds is condemned along with it.
The two are different instructions: any revocation means ask for a replacement, but only a compromised key means the key pair has to be discarded first.
Everything else can be re-requested with the key the application already holds.

Where the reason given is that the key is compromised, that key is not certified again, for any DNS name, by any application, since a leaked key is leaked whoever asks next.
A request naming it is refused distinguishably from every other refusal, so an agent can generate a fresh key and ask again on the strength of the refusal alone, without a human reading it and without waiting for an operator to intervene on the application.
Recovering from a leaked key is exactly the moment when nobody has attention to spare, so it is the moment the machinery has to work unattended.
Any other reason leaves the key usable, since a certificate superseded or a group retired says nothing about the key itself.

### When issuance fails

An order that fails is retried, with the interval between attempts growing, because most failures are the authority being briefly unavailable or a record not yet visible.

A certificate that is running out is a fact about the application that serves it, so it is filed against that application like any other check: it joins that application's group's incident and reaches the people who run that group (see [CHK](../monitoring/checks.md)).
It warns while there is still room to recover and fails as the remaining life runs down, and both thresholds are fractions of the certificate's own lifetime rather than fixed durations — otherwise the same alert would fire far too late for a short-lived certificate and far too early for a long-lived one.
A certificate that has expired outright fails regardless.

A paused application raises none of this either, for the same reason: Canopy has been told to stop acting on its behalf, so a certificate running down is the expected consequence rather than a failure. What is reported instead is the pause, and eventually the pause having been forgotten.

Except that a certificate for a DNS name the application is no longer entitled to raises nothing at all, however far past expiry it is.
Its group may have released the domain it sat under, its application may have released the DNS name for certificates, the application's grant may have been revoked, or the application may have been archived — and in each case Canopy deliberately stopped renewing it, so its running out is the intended outcome rather than a failure to report.
Alerting on it would mean every deliberate withdrawal left an alert behind that no action could clear, which teaches an operator to ignore the alert that matters.
Whether the DNS name is still entitled is asked when the alert is evaluated rather than remembered from when renewal stopped, so a domain reclaimed by its group brings its certificates back into scope.

An order that has never produced a certificate is distinguished from one extending a certificate that already exists, so an operator can tell a group that never came up from one about to go dark.

Canopy's own inability to issue is not any one application's fault and is reported against Canopy instead (see [SELF](../private-server/self-alerts.md)): an authority that cannot be reached, an account Canopy cannot use, and the authority's rate limits being exhausted.
Those limits are shared across every group whose domain sits in the same zone, so running them down is a fleet-wide fault rather than one group's: Canopy reports being throttled, and does not consume what remains retrying a DNS name that has just failed.
Reporting the two apart matters because they call for different people — an application's certificate running out is that group's problem to notice, and Canopy being unable to issue at all is Canopy's.

## Presentation

DNS names and TLS certificates are presented in separate sections wherever they appear, and each section carries only its own kind.
A DNS names section never shows a certificate or a request for one, and a TLS certificates section never shows an address or a request for one.
An application, a machine, and a group each present the two sections the same way, differing only in which names the section covers, so the two kinds read alike wherever they are met.
A section with nothing to show is absent.

### DNS names

An application presents the DNS names it declares for addresses, with the addresses published for each and whether the zone has caught up with what it asked for.
An operator declares and releases an application's DNS names for addresses from the same place.
A declared DNS name with no addresses registered presents as declared without addresses, distinct from one whose addresses are being withdrawn.
A declared DNS name outside every domain the group controls presents flagged as such, since nothing can be published for it until a covering domain is claimed.

A group presents, under each domain it controls, the DNS names declared for addresses beneath it and whether each has its records published, so whether a group's DNS names are healthy is answerable without visiting each of its applications.

A machine hosting several applications presents their DNS names together, since that is where an address request is resolved to one of them.
Each shows the application declaring it.
Any machine with undeclared address requests presents them, each with when it was last asked, with a control to declare the DNS name for addresses on one of the machine's applications and a control to deny address requests about it.
A machine hosting one application has them only for a DNS name another application holds, and declaring there is refused with the holder named, which is what an operator needs to release it first.
Declaring from the machine is the same declaration as declaring from the application, refused the same way.
A machine presents the DNS names denied to it for addresses, each with who denied it, when, and the note, and a control to lift the denial.

### TLS certificates

An application presents the DNS names it declares for certificates and the certificates Canopy holds for it, each with the DNS name it covers, the profile it was issued under, its state, and how long is left before it expires.
An operator declares and releases an application's DNS names for certificates from the same place.
A declared DNS name outside every domain the group controls presents flagged as such, since nothing can be certified for it until a covering domain is claimed.
A request that has not yet produced a certificate presents as pending, or as failed with the reason.
An operator sets the application's profile where its other permissions are set, and pauses or unpauses it from the same place, a pause showing who set it, when, and why.

How long is left is shown once, as a duration in a unit that suits its size, with the exact instant available from it, rather than as an instant beside a duration that says the same thing.
It is rounded the same way wherever it is shown.
It is coloured by how urgent it is, on the same measure as the certificate's state: relative to the certificate's own lifetime and renewal point rather than to a fixed number of days, so that a week left reads as calm on a ninety-day certificate and as urgent on a six-day one.
It is calm while the certificate is valid, draws attention once it is due for renewal, and reads as urgent once it is expiring or expired.

A group presents, under each domain it controls, the DNS names declared for certificates beneath it and which of them hold a current certificate, so whether a group's certificates are healthy is answerable without visiting each of its applications.

A machine hosting several applications presents their DNS names declared for certificates together, since that is where a certificate request is resolved to one of them.
Each shows the application declaring it and the state of its certificate.
Any machine with undeclared certificate requests presents them, each with when it was last asked, with a control to declare the DNS name for certificates on one of the machine's applications and a control to deny certificate requests about it.
A machine hosting one application has them only for a DNS name another application holds, and declaring there is refused with the holder named.
Declaring from the machine is the same declaration as declaring from the application, refused the same way.
A machine presents the DNS names denied to it for certificates, each with who denied it, when, and the note, and a control to lift the denial.

### Notices

Undeclared requests are surfaced as a notice rather than as a check: it is shown to whoever reads the pages it appears on and reaches no notification channel, since what it asks for is an operator's decision rather than a response to something down.
A group presents a notice while any of its machines has an undeclared request, saying how many there are, of which kind, and on which machines, and leading to each machine.
The Status page presents one notice across the fleet while any machine has an undeclared request, saying how many there are, of which kind, and in which groups, and leading to each group.
A notice goes once the requests it counts are declared, denied, or drop off.

### Issuance authority

The authority Canopy is configured to use is presented to operators along with the profiles it advertises and whether Canopy's account with it is usable, since that is where a misconfiguration of issuance shows up rather than on any one application.
