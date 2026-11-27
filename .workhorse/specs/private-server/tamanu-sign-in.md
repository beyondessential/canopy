---
id: TSI
---

# Signing in to Tamanu

Applies to the tokens Canopy issues for a person to sign in to one Tamanu central or facility: where sign-in is offered, what a token carries, how it reaches the Tamanu, and what the agent on the Tamanu's box checks before the Tamanu accepts it.

## Where it is offered

Sign-in is offered beside each address of a Tamanu central or facility, in the [projects view](../servers/projects.md) and on the application's page.
It is offered for an application of any rank.

Sign-in is offered on an address when all of these hold:

- the address is the application's tailnet address, or its public address served over HTTPS;
- the application is live, and runs on a machine whose bound identity holds an active enrolled key (see [DTR](device-trust.md));
- Canopy holds the key the application's reporter names it by (see [STA](../public-server/statuses.md), "Identifying an application").

> [!NOTE]
> A token is encrypted to the box's enrolled key and names its application by the reporter's key, so a Tamanu without either has nothing to open a token with or to match it against. An application on a cluster has no machine, and so no enrolled key.

Signing in is open to callers at the PM level and above (see [ADM](access-levels.md)).
It is graded read-only, since it changes nothing Canopy holds (see [SAFE](safety-modes.md)).

## Signing in

Choosing sign-in issues a token at that moment and opens the Tamanu's sign-in address in a new tab, carrying it.
Each choice issues a token of its own, and showing a sign-in control issues nothing.

The sign-in address is `<origin>/login?token=<token>`, the origin being the scheme, host and port of the address sign-in was chosen beside.

When Canopy cannot sign, or the machine's enrolled key cannot be read, sign-in fails and says which.

## The token

### Signed token

The token is a JSON Web Token signed with ES256 by Canopy's signing key, chosen as for identity tokens (see [IDP](identity-provider.md), "Signing keys").
Its header names the signing key, and its type is `canopy-sign-in+jwt`.

It carries:

- `iss`, the Canopy instance's issuer;
- `iat`, `nbf` and `exp`, the token living for one minute;
- `jti`, unique to the token;
- `sub`, `person:<login>`, the person's tailnet login in lowercase;
- `email`, the person's tailnet login;
- `name`, the person's display name as the tailnet last gave it;
- `origin`, the origin of the sign-in address the token is sent to;
- `application_key` and `application_type`, the key and type the application's reporter names it by.

> [!NOTE]
> The type and the subject's `person:` prefix keep a sign-in token from ever being taken for an [identity token](../public-server/identity-tokens.md), whose subjects name machines and applications, or the reverse.

### Encryption

The signed token is sent encrypted, as a compact JWE to the active enrolled key of the machine's bound identity, the most recently enrolled where it holds several.
The encryption uses `ECDH-ES` key agreement and `A256GCM` content encryption, with a content type of `JWT`, and its header names the key it was encrypted to by that key's JWK thumbprint.

A box still holding only an older key cannot open the token, and sign-in fails at the box until the box holds the key Canopy encrypted to.

> [!NOTE]
> Only the box holding the key can open the token, so a token that leaks from an access log, a browser history or a referrer opens nowhere else. Only the box's own Tamanu, at the origin it was sent to, within its minute and once, can use it.

## Accepting a token

A Tamanu receiving a sign-in token hands it to the agent on its box, with the origin it received the token at.
The agent accepts the token only when all of these hold:

- it decrypts with the box's enrolled key the token names;
- the signature verifies against the key set Canopy publishes at its issuer, `iss` is that issuer, and the type is a sign-in token's;
- the present moment is between `nbf` and `exp`;
- `origin` is the origin the Tamanu received the token at;
- `application_key` and `application_type` name the Tamanu that handed it over;
- the agent has not accepted a token with that `jti` before.

The agent remembers each token it accepts until that token expires, across its own restarts, so a token is accepted once.
On accepting a token, the agent gives the Tamanu the person's `sub`, `email` and `name`.

## Recording

Issuing a sign-in token is an audited action (see [AUD](../platform/audit-log.md)), and an important one.
Its actor is the person signing in, by their tailnet login.
It acted on the application and its machine.
Its detail is the token's identifier, the signing key, the enrolled key it was encrypted to, the expiry, the origin and the subject.
The token itself is never recorded.

> [!NOTE]
> Every sign-in passes through Canopy, so the log is a record of who signed in to which Tamanu and when.

The entry is written with the issuance, before the token is returned, and a token whose entry cannot be written is not issued.
These entries are kept for the log's default retention.

## Borrowed terms

- **JWE**: a JSON Web Encryption object, as defined by [RFC 7516](https://www.rfc-editor.org/rfc/rfc7516), with its algorithms named as in [RFC 7518](https://www.rfc-editor.org/rfc/rfc7518).
- **JWK thumbprint**: a hash identifying a public key, as defined by [RFC 7638](https://www.rfc-editor.org/rfc/rfc7638).
