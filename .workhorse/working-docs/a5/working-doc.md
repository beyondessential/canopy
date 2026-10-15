---
status: draft
---

# Canopy as OIDC IdP for workload identity

Exploration of whether Canopy can act as an OIDC identity provider so canopy-authed machines and apps can federate into Anthropic (workload identity federation for `ANTHROPIC_API_KEY`) and other relying parties, governed by per-machine and per-application policy in Canopy.

## Constraints from Anthropic WIF

- `iss` must equal the registered issuer URL byte for byte. Discovery mode needs a public `https:443` hostname serving `/.well-known/openid-configuration` and a JWKS. `explicit_url` and `inline` relax this; `inline` means manual key pushes on rotation.
- JWT: asymmetric only (ES256 fine), `kid` header, `sub`, `iat`, `exp` present. Lifetime within the issuer maximum (1h default). `jti` is single-use by default, so every exchange needs a fresh JWT.
- Rules match on `sub` prefix, `aud`, exact top-level claims, or CEL over the full claim set. Whatever claims Canopy emits are what Anthropic rules key on.
- Minted Anthropic token lifetime is the lesser of the rule's lifetime and twice the JWT's remaining life.
- New signing keys must be published in the JWKS at least 15 minutes before first use; keep the old key until its tokens expire.
- SDKs re-read `ANTHROPIC_IDENTITY_TOKEN_FILE` on every exchange, or accept a token-provider callback. A stale file causes `jti_reused`.
- Anthropic-side issuer, service account and rule are configured in the Console or Admin API. Not Canopy's concern for now.

## Behaviour

Sketch only.

- Public, unauthenticated discovery document and JWKS on the public server.
- An authenticated device endpoint (working name `POST /machines/self/identity-token {audience}`) returning a short-lived JWT. The device is resolved from its certificate, as for backup credentials.
- Issuance is gated by an operator grant of audience to a machine, application or group, and is refused without one. Issuance is recorded before the token is returned.
- Claims carry the machine, group, rank and application types so a relying party can scope its own rules.

## Implementation options

### Signing key custody

Decided direction: something in Kubernetes if possible (Canopy runs there), AWS KMS otherwise.

- Kubernetes Secret holding a keyset (active key plus published-only next key). No new dependency; key is extractable from the process and by anyone who can read the Secret.
- AWS KMS asymmetric key (ECC_NIST_P256). Non-exportable; Canopy only calls Sign. Needs KMS reachable from the public server.

### Kubernetes workloads

Decided: A now, B later. A and B are each split out to their own card in the breakdown.

Pods have no Canopy identity; only the cluster relay is enrolled. Options:

- A: the cluster's own OIDC issuer registered directly with Anthropic. No Canopy work; policy lives only in Anthropic rules.
- B: relay-mediated exchange. A pod presents its projected service-account token, the relay performs the TokenReview, and Canopy issues its own JWT with the namespace's group and rank. One issuer and one policy and audit surface across machines and pods. Keeps the rule that Canopy holds no credential to the cluster.

### Delivery to a box

Decided: bestool exposes a local HTTP endpoint the workload calls, and bestool calls Canopy with the device key. Each call yields a fresh token, so `jti` reuse does not arise and the workload never holds the key. Stays in this card's design. What Canopy owes it is the device endpoint above.

### Issuer hostname

The issuer is a dedicated name of our own (working example `https://idp.canopy.bes.au`, name to be workshopped), independent of the public server's device-facing host, since `iss` can never change without re-registering every relying party.

It need not be routable for Anthropic: with `jwks.discovery_base` (or `explicit_url`) set, Anthropic compares `iss` as a string and fetches discovery and JWKS from the public server instead.
Other relying parties (AWS IAM OIDC providers, for one) fetch discovery from the issuer URL itself, so being generic means the name eventually resolves and serves just the discovery document and JWKS.
Either way the domain must be one we hold, so nobody else can ever serve discovery for it.

### Grant model

Generic from the start: a grant names an audience, not Anthropic specifically.

Decided: grants are their own thing, held against a machine or against an application, not the `Scope` used by check policy (no group, cluster or Canopy-wide grants).

A token is requested for one of two subjects, and each needs its own grant for the audience:

- **The machine itself.** `sub` names the machine. Needs a machine grant.
- **An application on the machine, named by type.** bestool asks for, say, a `tamanu-central` token on its own box. `sub` names that application. Needs a grant on that application.

A machine grant does not cover the applications on the box, and an application grant does not cover its machine: they are different principals to a relying party, and a box carrying two workloads should not let one stand in for the other.
Like the DNS and TLS permissions, grants are withheld by default and a missing one is refused as a missing permission, not as a silence.
Storage follows the one-nullable-FK-per-grain pattern (`machine_id`, `application_id`, exactly one set) so deletion cascades, but with its own two-variant target type rather than `Scope`.

Whatever the grant shape, audiences are a registry of relying parties an operator adds (name, audience string, token lifetime), and grants reference an entry rather than free text. That avoids typos silently granting nothing, and lets the interface say "may obtain tokens for Anthropic".

Claims carry Canopy's facts regardless (group, rank, application type, machine), so a relying party can still route further, for example production to one Anthropic workspace and test to another. Claims come only from Canopy-derived facts, never from operator-set tags, so editing a tag can never widen what a relying party accepts.

## Open questions

- [ ] A box carrying two applications of one type: is naming the type ambiguous, and does the request then name the application's report key instead?
- [ ] Does an archived or unreachable application keep its grant (and can it still get tokens while unreachable)?
- [ ] Issuer name: final hostname.

## Trade-offs

- Grants by group, rank or type across the fleet (reusing `Scope` with filters) were considered and rejected: a grant is a deliberate act on one machine or application, like the DNS and TLS permissions.
- Attest-only (Canopy issues to any enrolled identity, relying parties do all policy) was rejected: the card wants the policy held in Canopy.

## Testing notes
