---
id: TSI
---

# Signing in to Tamanu

Applies to the tokens Canopy issues for a person to sign in to one Tamanu central or facility: where sign-in is offered, what a token carries, how it reaches the Tamanu, and what the agent on the Tamanu's box checks before the Tamanu accepts it.

A sign-in token grants administrative access to the Tamanu it is issued for.
The Tamanu signs its bearer in as its own administrator, by whatever account it keeps for that, and what the person does from there is the Tamanu's to govern.
The person is named in the token for the site's own records.

> [!NOTE]
> A Tamanu administrator can act as any of its users, so a sign-in is administrative access whatever account it lands on. That is why sign-in is kept to the PM level and above, and why Canopy records every token it issues.

## Where it is offered

Sign-in is offered beside a Tamanu central's or facility's address: beside each of its two addresses in the [projects view](../servers/projects.md), and beside the one link its own page offers (see [SVC](service-links.md), "Application link").
It is offered for an application of any rank.

Sign-in is offered on an address when all of these hold:

- the address is the application's tailnet address, or its public address served over HTTPS;
- the Canopy instance has an issuer and a signing key configured (see [IDP](identity-provider.md), "Configuration");
- the application is live, and runs on a machine whose bound identity has authenticated over mutual TLS with a key that is still active (see [DTR](device-trust.md));
- Canopy holds the key the application's reporter names it by (see [STA](../public-server/statuses.md), "Identifying an application").

> [!NOTE]
> A token is encrypted to the key the box authenticates with and names its application by the reporter's key, so a Tamanu without either has nothing to open a token with or to match it against. An application on a cluster has no machine, and so no such key.

Signing in is open to callers at the PM level and above (see [ADM](access-levels.md)).
It is graded read-only (see [SAFE](safety-modes.md), "Grading the operator surface"): it can be undone, it writes nothing to the fleet or to Canopy's records, and the token it issues is the caller's own, for one Tamanu, for a minute.

## Signing in

Choosing sign-in issues a token at that moment and opens the Tamanu's sign-in address in a new tab, carrying it.
Each choice issues a token of its own, and showing a sign-in control issues nothing.

The sign-in address is `<origin>/login?token=<token>`.
Its origin is that of the address sign-in was chosen beside, serialised as an origin is in RFC 6454: the scheme and host in lowercase, and the port only where it is not the scheme's default.
A configured URL's path is not part of its origin.

Sign-in is limited per person and, separately, per Tamanu, so a client or script stuck in a loop cannot flood the audit log or the key service.
A sign-in over either limit is refused as rate limited, and reaching either limit raises a self-alert (see [SELF](self-alerts.md)).
The limits are set so that no person signing in by hand reaches them.

When sign-in is refused, or Canopy cannot sign or encrypt the token, the view sign-in was chosen in says why, and no tab is opened.

## The token

### Signed token

The token is a JSON Web Token signed with ES256 by Canopy's signing key, chosen as for identity tokens (see [IDP](identity-provider.md), "Signing keys").
Its header names the signing key, and its type is `canopy-sign-in+jwt`.

It carries:

- `iss`, the Canopy instance's issuer;
- `iat`, when it was issued, `nbf`, equal to `iat`, and `exp`, one minute later;
- `jti`, unique to the token;
- `sub`, `person:<login>`, the person's tailnet login in lowercase;
- `origin`, the origin of the sign-in address the token is sent to;
- `application_key` and `application_type`, the key and type the application's reporter names it by.

> [!NOTE]
> The type and the subject's `person:` prefix keep a sign-in token from ever being taken for an [identity token](../public-server/identity-tokens.md), whose subjects name machines and applications, or the reverse.

### Encryption

The signed token is sent encrypted, as a compact JWE to the active key the machine's bound identity most recently authenticated with over mutual TLS.
The encryption uses `ECDH-ES` key agreement and `A256GCM` content encryption, with a content type of `JWT`, and its header names the key it was encrypted to by that key's JWK thumbprint.

> [!NOTE]
> The key the box last authenticated with is the one its agent holds. A device can hold other active keys, such as one provisioned for a person to carry elsewhere (see [DPK](provisioned-credentials.md)), which the box cannot open a token with.

> [!NOTE]
> Encryption keeps the token's claims private and binds it to one box, not to one browser: whoever holds the sign-in address can use it at that Tamanu within its minute, if the tab it was opened for has not used it first.

## Accepting a token

The agent is the Canopy agent on the Tamanu's box, which holds the key the box authenticates to Canopy's device API with.
A Tamanu receiving a sign-in token hands it to the agent before it renders anything carrying the sign-in address, so the address cannot leak onward, for instance in a referrer.
The agent tells which Tamanu on the box handed it the token.

The agent learns Canopy's issuer from Canopy over the device API, and verifies against the key set Canopy publishes at that issuer.
A token naming a signing key the agent does not hold in its copy of the key set makes it fetch the key set again before deciding.

The agent accepts the token only when all of these hold, and otherwise refuses it for the first reason that applies:

1. it decrypts with a key the box holds, or it is refused as not for this box;
2. the key set can be fetched, or it is refused as unverifiable for now;
3. the signature verifies against the key set, `iss` is Canopy's issuer, and the type is a sign-in token's, or it is refused as not signed by Canopy;
4. the agent's clock reads no earlier than thirty seconds before `nbf` and no later than thirty seconds after `exp`, or it is refused as expired or not yet valid;
5. `origin` is one of the origins the Tamanu is configured to serve, or it is refused as for another address;
6. `application_key` and `application_type` name the Tamanu that handed it over, or it is refused as for another Tamanu;
7. the agent has not accepted a token with that `jti` before, or it is refused as already used.

The agent remembers each token it accepts until thirty seconds after that token's `exp`, across its own restarts, so a token is accepted once.
On accepting a token, the agent tells the Tamanu so, and may give it the token's `sub` for the site's own records.
On refusing one, it gives the Tamanu the reason, which the Tamanu shows the person.

> [!NOTE]
> The origin is compared with the Tamanu's own configuration rather than with the address a request arrived at, because a request's address can be relayed: a host set as an application's URL in Canopy could forward a token to the real Tamanu under its own name.

## Recording

Issuing a sign-in token is an audited action (see [AUD](../platform/audit-log.md)), and an important one.
Its actor is the person signing in, by their tailnet login.
It acted on the application and its machine.
Its detail is the token's identifier, the signing key and the expiry, recorded as an identity token's issuance records them (see [IDT](../public-server/identity-tokens.md), "Recording"), with the key it was encrypted to, the origin and the subject.
The token itself is never recorded.

> [!NOTE]
> The log is a record of who was issued a sign-in token for which Tamanu, and when. Whether the Tamanu accepted it is in the site's own records.

The entry is written with the issuance, before the token is returned, and a token whose entry cannot be written is not issued.
A sign-in refused as rate limited is not recorded.
These entries are kept for the log's default retention.

## Borrowed terms

- **JWE**: a JSON Web Encryption object, as defined by [RFC 7516](https://www.rfc-editor.org/rfc/rfc7516), with its algorithms named as in [RFC 7518](https://www.rfc-editor.org/rfc/rfc7518).
- **JWK thumbprint**: a hash identifying a public key, as defined by [RFC 7638](https://www.rfc-editor.org/rfc/rfc7638).
- **Origin**: the scheme, host and port a web address is served from, as defined by [RFC 6454](https://www.rfc-editor.org/rfc/rfc6454).
