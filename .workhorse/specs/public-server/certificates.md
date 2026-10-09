---
id: CRT
---

# Application TLS certificates

An application asks Canopy to obtain a TLS certificate for one of its DNS names, because Canopy is the only holder of the certificate authority account and the only party that can prove control of the DNS name through the zone.
The request is refused unless an operator has granted the application permission to obtain certificates, and is confined to DNS names within the domains the application's group controls (see [DOM](../servers/domains.md)).
Which application a request is about, what is recorded when that cannot be settled, and how an operator declares and denies DNS names for certificates are common to addresses and are in [DNS](dns-names.md).
Certificates are declared, requested, recorded, denied, and presented without reference to addresses: an application may hold a certificate for a DNS name it has registered no addresses for, and the reverse.

## Why Canopy issues

An application's DNS name resolves to an address that may not be reachable from the public internet, and often is not: a facility application sits behind someone else's NAT.
So the challenge types that prove control by answering on the DNS name's own address are unavailable, and proving control through DNS is the only route left.
Proving control through DNS means writing to the zone, and Canopy already holds that access on the group's behalf.

Centralising issuance also puts the authority's rate limits, the record of every certificate's expiry, and the alerting when renewal stops working in one place — the same place that already watches the fleet.

## Requesting

An application generates its own key pair and asks Canopy to certify it, submitting a certificate signing request for a single DNS name.
The private key never leaves the application and Canopy never asks for it: Canopy's part is to prove control of the DNS name and return the signed chain.

The signing request is honoured only for exactly the DNS name requested.
Canopy certifies that one DNS name and no other: a request whose subject or alternative names carry anything beyond the requested DNS name is refused rather than trimmed, because silently issuing something narrower than asked would leave an application serving a certificate it does not expect, and issuing something wider would let one application smuggle another group's DNS name past the authorisation check.
Wildcards are refused: a certificate valid for every DNS name in a group is not something one member should be able to mint.

The key the request certifies must be strong enough to be worth certifying, and Canopy states what it accepts rather than deferring to whatever the authority happens to allow that year.

## Fulfilment is not immediate

Proving control through DNS takes as long as it takes for the authority to see a record Canopy has just published — tens of seconds at best, minutes when a resolver holds a negative answer.
That is far longer than any client will wait mid-handshake, so requesting a certificate and collecting it are separate steps: a request is accepted and acknowledged, Canopy works the order in the background, and the application collects the result when it is ready.

An application therefore holds a certificate before it needs one, rather than obtaining one at the moment a client arrives.
Canopy's contract is only that a request is durable once accepted and that its outcome becomes collectable; scheduling requests early enough to be useful is the application's business.

Repeating a request for a DNS name Canopy already holds a valid certificate for returns the one it holds rather than ordering another, so an application that has lost its local copy — restarted, redeployed, cache cleared — is served without spending the authority's budget.
A request naming a key different from the one already certified is a new order, since the stored chain certifies a key the application no longer holds.

## What Canopy keeps

Canopy keeps the certificate it obtained, the DNS name it covers, the application it was issued for, and when it expires.
It keeps no private key, having never held one.

Holding the chain is what lets Canopy answer a repeat request without a fresh order, renew before expiry without being asked, and report a certificate that is running out.

## Lifetime

An authority may offer certificates of more than one lifetime, named as profiles, and an application's certificates are requested under one of them.
The profiles on offer are whatever the authority advertises, so Canopy presents that set rather than a list of its own, and a profile the authority has withdrawn is reported as unavailable instead of being requested and refused.

An application's profile is an operator's choice per application, because lifetime is a property of how an application is run rather than of Canopy: a cloud-hosted one whose issuance is exercised constantly can carry a short lifetime, where an on-premises one that may be offline for days cannot.
Every application takes the longest profile the authority offers until an operator says otherwise, so a short lifetime is something adopted deliberately for an application rather than a default anyone inherits.

## Renewal

Canopy renews a certificate it holds before it expires, without being asked, and the renewed chain becomes collectable the same way the first one did.
An application that collects periodically therefore stays current without tracking expiry itself, and one that asks again gets whatever is newest.

When to renew comes from the authority when it will say: an authority that publishes renewal information is asked when it would like this certificate replaced, and Canopy renews in the window it names.
Failing that, Canopy renews after a fixed fraction of the certificate's own life has passed.
Neither is a fixed interval, because a fixed interval cannot serve both lifetimes: a window measured in weeks would leave a certificate that lives days permanently overdue, and one measured in hours would renew a long-lived certificate hundreds of times over.
Where the authority accounts for a renewal as replacing a particular certificate, Canopy tells it which, so a renewal is not mistaken for an additional certificate.

Renewal stops when the certificate is no longer wanted: a DNS name whose group has released the domain it sits under is not renewed, nor is a certificate for a DNS name its application no longer declares for certificates, or for an application whose grant has been revoked or that has been archived.
A grant revoked does not withdraw the certificate already issued — it cannot be recalled once it exists — but it does end the renewals that would extend it.

## Revocation

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

## When issuance fails

An order that fails is retried, with the interval between attempts growing, because most failures are the authority being briefly unavailable or a record not yet visible.

A certificate that is running out is a fact about the application that serves it, so it is filed against that application like any other check: it joins that application's group's incident and reaches the people who run that group (see [CHK](../monitoring/checks.md)).
It warns while there is still room to recover and fails as the remaining life runs down, and both thresholds are fractions of the certificate's own lifetime rather than fixed durations — otherwise the same alert would fire far too late for a short-lived certificate and far too early for a long-lived one.
A certificate that has expired outright fails regardless.

A paused application (see [DNS](dns-names.md), "Pausing an application") raises none of this either, for the same reason: Canopy has been told to stop acting on its behalf, so a certificate running down is the expected consequence rather than a failure. What is reported instead is the pause, and eventually the pause having been forgotten.

Except that a certificate for a DNS name the application is no longer entitled to raises nothing at all, however far past expiry it is.
Its group may have released the domain it sat under, its application may have released the DNS name for certificates, the application's grant may have been revoked, or the application may have been archived — and in each case Canopy deliberately stopped renewing it, so its running out is the intended outcome rather than a failure to report.
Alerting on it would mean every deliberate withdrawal left an alert behind that no action could clear, which teaches an operator to ignore the alert that matters.
Whether the DNS name is still entitled is asked when the alert is evaluated rather than remembered from when renewal stopped, so a domain reclaimed by its group brings its certificates back into scope.

An order that has never produced a certificate is distinguished from one extending a certificate that already exists, so an operator can tell a group that never came up from one about to go dark.

Canopy's own inability to issue is not any one application's fault and is reported against Canopy instead (see [SELF](../private-server/self-alerts.md)): an authority that cannot be reached, an account Canopy cannot use, and the authority's rate limits being exhausted.
Those limits are shared across every group whose domain sits in the same zone, so running them down is a fleet-wide fault rather than one group's: Canopy reports being throttled, and does not consume what remains retrying a DNS name that has just failed.
Reporting the two apart matters because they call for different people — an application's certificate running out is that group's problem to notice, and Canopy being unable to issue at all is Canopy's.

## Presentation

The TLS certificates section of an application, a machine, and a group is described in [DNS](dns-names.md), "Presentation"; this is what it carries for certificates.

An application presents the DNS names it declares for certificates and the certificates Canopy holds for it, each with the DNS name it covers, the profile it was issued under, its state, and how long is left before it expires.
A request that has not yet produced a certificate presents as pending, or as failed with the reason.
An operator sets the application's profile where its other permissions are set, and pauses or unpauses it from the same place, a pause showing who set it, when, and why.

How long is left is shown once, as a duration in a unit that suits its size, with the exact instant available from it, rather than as an instant beside a duration that says the same thing.
It is rounded the same way wherever it is shown.
It is coloured by how urgent it is, on the same measure as the certificate's state: relative to the certificate's own lifetime and renewal point rather than to a fixed number of days, so that a week left reads as calm on a ninety-day certificate and as urgent on a six-day one.
It is calm while the certificate is valid, draws attention once it is due for renewal, and reads as urgent once it is expiring or expired.

A group presents, under each domain it controls, the DNS names declared for certificates beneath it and which of them hold a current certificate, so whether a group's certificates are healthy is answerable without visiting each of its applications.

A machine's TLS certificates section carries the DNS names declared for certificates, each with the application declaring it and the state of its certificate, the undeclared certificate requests, and the DNS names denied for certificates, and nothing about addresses.

## Issuance authority

The authority Canopy is configured to use is presented to operators along with the profiles it advertises and whether Canopy's account with it is usable, since that is where a misconfiguration of issuance shows up rather than on any one application.
