---
id: IDP
---

# Identity provider

Canopy is an OpenID Connect issuer for its fleet.
It publishes the keys its identity tokens are signed with, keeps the register of relying parties those tokens are issued for, and holds which machines and applications may obtain tokens for each.
How a machine requests a token, and what the token carries, are in [IDT](../public-server/identity-tokens.md).

## Configuration

The issuer and the signing keys come from the Canopy instance's configuration, set by the infrastructure that provisions Canopy, rather than from operator-editable state.
Each Canopy instance has an issuer of its own, so a token from one instance is never accepted where another's is trusted.
The issuer is a name of its own, apart from the device API's, because it is fixed in every relying party's trust configuration while the device API is free to move.
Canopy reads the configuration when it starts, so a change takes effect once Canopy restarts.

A Canopy instance with no issuer or no signing key configured issues no tokens.
Its identity provider settings present that, and the grant controls are not offered.

## Discovery

The discovery document is served at `<issuer>/.well-known/openid-configuration`, and the key set it names is served beside it, both at the issuer name itself.
So a relying party registers Canopy in ordinary discovery mode, finding the documents where it expects them and checking the document's issuer against where it fetched it.

The two documents are served by a component of their own that serves nothing else.
It is unauthenticated, holds no access to Canopy's database and no ability to sign, and builds the key set from the public halves of the configured keys.
The only internet-facing part of the identity provider therefore holds nothing secret.

## Signing keys

Signing keys are P-256 keys held by a managed key service that signs on Canopy's behalf and never releases the private key, so no copy of a signing key exists anywhere for Canopy to lose.

Every configured key is published in the key set.
Canopy signs with the newest configured key that has matured, a key maturing once it has been served at the issuer for long enough that relying parties' cached key sets include it.
The first key a Canopy instance is configured with is mature from the outset, there being no earlier key for relying parties to rely on meanwhile.
Signing only moves forward: once Canopy has signed with a key, it never signs with an older one.
Canopy judges maturity from the key set as actually served at the issuer, which it fetches and observes, rather than from its own configuration, because the discovery component and the device API take up a configuration change separately.
The moment Canopy first observes a key served is recorded once and stands.

A key that no longer signs can be removed once every token it signed has expired, and Canopy presents that moment for each key, so removing a key from the configuration follows Canopy's record rather than an estimate.
Each key is presented with whether it is signing, published and waiting to sign, or removable, when it was first served, when it last signed, and how many tokens it signed in the last thirty days.
Those figures are derived from the audit log's record of issuance (see [IDT](../public-server/identity-tokens.md), "Recording").

When the documents served at the issuer cannot be fetched, name a different issuer, or carry a key set that differs from the configured keys, Canopy raises a self-alert (see [SELF](self-alerts.md)).

## Relying parties

An operator registers each relying party with a name, its audience, and the lifetime of the tokens issued for it.
The name is what operators see; the audience is what a token carries and what a machine names when it asks for one, so two relying parties never share an audience.
A token lifetime defaults to five minutes and is at most an hour.

The register presents each relying party with its audience, its token lifetime, how many machines and applications hold a grant for it, and when a token was last issued for it.

Removing a relying party revokes every grant for it, and a request naming its audience is then refused as unknown.

## Grants

A grant lets one machine, or one application, obtain tokens for one relying party.
Grants are withheld by default, and a subject without one is refused and told so (see [IDT](../public-server/identity-tokens.md), "Refusals").

A machine's grant covers tokens naming the machine itself, and an application's covers tokens naming that application.
Neither covers the other, because a relying party sees them as different principals, and a machine carrying several workloads must not let one stand in for another.

Revoking a grant refuses the subject's next request; tokens already issued run until they expire.
An archived machine or application keeps its grants but obtains nothing while archived, since an archived machine cannot authenticate and an archived application is not live, and restoring it returns what it held.
Granting and revoking are administrative actions recorded in the audit log, with who acted and when.

## Presentation

The identity provider has its own page among Canopy's settings, presenting the issuer, the discovery document's address, the signing keys, and the register of relying parties, where relying parties are added and removed.

A machine's edit page offers, for the machine and for each of its applications, a searchable choice of the registered relying parties it may obtain tokens for, beside the other permissions an operator holds over it.
An application's choice also shows the subject its tokens carry, ready to copy into a relying party's rule.
The choice is not offered while no relying party is registered, unless the machine or application already holds a grant, so a held grant can always be withdrawn.
