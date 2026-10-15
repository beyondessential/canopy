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

Decided: A now, B later. Split out to its own card in the breakdown.

Pods have no Canopy identity; only the cluster relay is enrolled. Options:

- A: the cluster's own OIDC issuer registered directly with Anthropic. No Canopy work; policy lives only in Anthropic rules.
- B: relay-mediated exchange. A pod presents its projected service-account token, the relay performs the TokenReview, and Canopy issues its own JWT with the namespace's group and rank. One issuer and one policy and audit surface across machines and pods. Keeps the rule that Canopy holds no credential to the cluster.

### Delivery to a box

Decided: bestool exposes a local HTTP endpoint the workload calls, and bestool calls Canopy with the device key. Each call yields a fresh token, so `jti` reuse does not arise and the workload never holds the key. Split out to its own card in the breakdown. What Canopy owes it is the device endpoint above.

## Open questions

- [ ] Issuer hostname: which public hostname, given the issuer URL can never change without re-registering every relying party?
- [ ] Generic audience-keyed grants from the start, or Anthropic-only first?
- [ ] Grant model: reuse the existing scope enum for grants, or a separate grant table keyed by audience?

## Trade-offs

## Testing notes
