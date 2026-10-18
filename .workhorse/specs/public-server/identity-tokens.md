---
id: IDT
---

# Identity tokens

A machine obtains from Canopy a short-lived signed identity token naming itself or one of its applications, to present to an external service that trusts Canopy as an OpenID Connect issuer.
That service, a relying party, verifies the token against the keys Canopy publishes and decides from its claims what the bearer may do, so a workload reaches it holding no long-lived credential of its own.
Canopy issues a token only to a subject an operator has granted that relying party; the issuer, its keys, the register of relying parties and the grants are in [IDP](../private-server/identity-provider.md).

## Requesting

A machine requests a token with `POST /identity-tokens`, authenticating with the machine role over either transport Canopy accepts (see [DTR](../private-server/device-trust.md)).

The request names the relying party by its audience: the string the relying party expects to find in the token, which is also what the workload's own configuration carries.
It may also name an application type.
Without one, the token's subject is the calling machine itself.
With one, the subject is the application of that type on the calling machine, named by type the way the DNS name endpoints name an application (see [DNS](dns-names.md)).

Each request mints a new token with an identifier of its own, and Canopy caches nothing between requests.
A relying party that accepts each token only once is therefore always given one it has not seen, and a client asks again whenever it needs a token rather than keeping one.
Requests are limited per caller, so a client stuck in a loop cannot flood the audit log or the key service.

The response carries the token, when it expires, and the subject it names, so a client can log what it was given without decoding the token.

## Refusals

A refusal tells the client whether waiting can help: a decision only an operator can change is told apart from a condition expected to clear.

Every request is checked in the same order, stopping at the first refusal, so the refusal a client sees is the same however many conditions hold:

1. A caller asking faster than Canopy accepts is refused as rate limited (429).
2. A caller that is not bound to a live machine is refused as on every machine endpoint (412).
3. An audience that matches no registered relying party is refused as unknown (404), apart from not granted, so a mistyped audience in a workload's configuration reads as a typo rather than as a missing grant.
4. An application type that names no live application on the calling machine is refused as no such subject (404); an archived application is not live.
5. A subject that holds no grant for the relying party is refused as not granted (403).
6. A subject whose machine is pending or belongs to no group is refused as not placed (409), naming what is missing, and the refusal clears once an operator ranks or groups the machine (see [GRP](../servers/groups.md), "Environments").
7. When Canopy cannot sign, because its key service is unavailable or because the Canopy instance has no issuer or signing key configured, the request fails as an upstream failure (502).

The rate limit comes first so a client stuck in a loop is held back whatever it asks for.
The grant comes before placement so a subject missing both learns first of the one waiting cannot clear.

## The token

A token is a JSON Web Token signed with ES256, whose header names the signing key it was signed with.

It carries:

- `iss`, the Canopy instance's issuer;
- `aud`, the relying party's audience;
- `iat`, `nbf` and `exp`;
- `jti`, unique to the token;
- `sub`, the structured subject below;
- `group_id`, `group_name`, `rank` and `machine_id`;
- for an application, `application_id` and `application_type`.

The claims beyond the registered ones are top-level strings without a prefix of Canopy's own, since the issuer already scopes them, so a relying party that matches only top-level string claims can match every one.
The values a rule should match on are identifiers and fixed vocabularies: `group_name` is carried for reading only, since a group can be renamed and a rename must not silently break a rule.
Every claim is derived from Canopy's own record of the subject and never from operator-set tags, so editing a tag cannot widen what a relying party accepts.

### Subject

An application's subject is `group:<group id>:rank:<rank>:application:<application type>:<application id>:`, and a machine's is `group:<group id>:rank:<rank>:machine:<machine id>:`.

The subject carries what a relying party able to match only on the subject needs to select a set of subjects by pattern: every production Tamanu central in one group, or every production Tamanu central in the fleet.
Its segments run broadest first, so a prefix selects a group, then an environment within it, then a kind of application within that.
Because the subject encodes group and rank, it changes when its machine moves group or rank, and a rule written for one environment stops matching a machine that leaves it.
The machine an application runs on and its group's name are carried in claims and never in the subject, so the subject changes only when something a rule selects on changes.

Every segment value is drawn from lowercase letters, digits and hyphens: identifiers, rank names, and application types, which are slugs (see [APP](../servers/application-types.md)).
So no value contains the separator, and a label appearing as a value cannot be mistaken for the label.
Each label appears once, in a fixed position, so a pattern's text between separators matches only the segment it was written for, even where a wildcard before it takes in several segments.
Canopy issues no token whose subject would carry a value outside that alphabet.

The trailing separator terminates every segment, the last included.
A pattern that ends at a separator therefore never matches a longer value sharing its prefix, and an exact match written without the trailing separator matches nothing, so a mistake in a rule refuses rather than admits.

A subject always carries its group and rank: a pending or ungrouped subject is refused rather than issued a token with a placeholder in their place.

## Lifetime

A token lives for the lifetime configured on its relying party (see [IDP](../private-server/identity-provider.md), "Relying parties").
A client exchanges a token with its relying party as soon as it obtains one, so the lifetime bounds how long a token that leaks stays useful rather than how long a workload can go without asking.

## Recording

Every issuance, and every refusal as not granted, is recorded in Canopy's audit log: the subject, the identity of the calling machine, the relying party, and for an issuance the token's identifier, signing key and expiry.
A refusal as not granted is a workload asking for access no operator has given it, which an operator may need to look into; every other refusal is a mistake or a passing condition the client reports for itself, and is not recorded.
The record is written before the response is returned, and a token whose record cannot be written is not issued, so Canopy can account for every token it has issued.
The token itself is never recorded.
