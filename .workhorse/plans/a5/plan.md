# Canopy as OIDC identity provider: plan

Specs: [IDT](../../specs/public-server/identity-tokens.md), [IDP](../../specs/private-server/identity-provider.md), and the identity provider condition in [SELF](../../specs/private-server/self-alerts.md).
Reasoning and rejected options: `.workhorse/working-docs/a5/working-doc.md`.

**Blocked on U2** (general audit log, [AUD](../../specs/platform/audit-log.md)). Grants history, issuance and not-granted refusal records, and the key usage figures all live in it.

## Tech design notes

### Signing

- AWS KMS asymmetric keys, `ECC_NIST_P256`, `ECDSA_SHA_256`. KMS returns DER signatures; JWS ES256 needs raw `r || s`, so convert.
- `kid` is the RFC 7638 JWK thumbprint of the public key, so it is derived, stable, and identical in the public server and the discovery server without coordination.
- IAM: the public server gets `kms:Sign` and `kms:GetPublicKey` on the configured keys; the discovery server gets `kms:GetPublicKey` only.
- Configuration (Pulumi, per Canopy instance): the issuer URL and the ordered list of key ARNs, newest last. Read at startup, as managed zones are.
- Tests and the e2e binary use an in-process P-256 key behind the same signer trait, as the backup Secrets use an in-memory store.

### Key maturation

- The jobs process fetches `<issuer>/.well-known/openid-configuration` and the JWKS on a short cadence and records, per `kid`, when it was first seen served.
- The signing key is the newest configured key first seen served at least 30 minutes ago (comfortably beyond Anthropic's 15-minute guidance). The first key ever configured is mature from the outset. Signing never moves to an older key than one already used, so persist the newest key signed with and never select below it.
- `first_served_at` is written once per `kid` and never updated.
- The same fetch drives the self-alert (working name `idp-discovery`, `canopy` source), with the differing `kid`s in the detail. Failed: unreachable, wrong issuer, or the signing `kid` missing from the served JWKS. Warning: any other difference from configuration.
- Key state: unserved (no observation), waiting (newer than the signing key), signing, retired or removable (older than the signing key; removable once the latest recorded expiry of its tokens has passed, or at once if it signed none). The UI collapses unserved, waiting and retired to "published".

### Audit

- Actions in U2's `database::audit::Action` enum: issue identity token, grant identity, revoke identity grant, register / edit / remove relying party. Grant, revoke and relying-party actions are kept for a year; issuance takes the default ninety days.
- Issuance: records acted on are the subject (machine or application) and the relying party; detail is `jti`, `kid`, `exp`, `sub`. The actor is the machine device, with the application it acted for set by the handler once `application_type` resolves, so U2's recorder must let the handler name it.
- A successful issuance records in the handler's transaction, not ahead: the token has no effect until it reaches the client, and the layer commits before responding. Depends on U2's layer committing before the response is sent and turning a failed commit into an error.
- Not granted: commit an issuance entry ahead and settle it failed at once with reason `not_granted`, since the 403 rolls back the handler's transaction. No other refusal records anything, so nothing is committed ahead on the other paths.
- No-op saves use AUD's explicit no-change outcome (decided for U2, see the comment there): a grants save or relying-party edit that changes nothing declares so rather than recording an entry.
- `MachineEdit`: add a grants call to the save plan only for a subject whose set changed, as the silence calls are, so an unchanged save sends nothing for grants.
- Route declarations: `POST /identity-tokens` is `audit(IssueIdentityToken)`; settings reads and grants reads are `silent`; each write declares its actions (remove declares both remove and revoke).
- Key "last signed" and thirty-day count, relying party "last issued": AUD's lookup and count by detail value over a window. The removable state reads the latest issuance per `kid`, always recent since a retired key's last token was issued within a lifetime of retirement.

### Discovery server

- New crate and image, working name `oidc-disco-server`. Serves only `/.well-known/openid-configuration` and the JWKS path.
- Discovery document: `issuer`, `jwks_uri`, `response_types_supported: ["id_token"]`, `subject_types_supported: ["public"]`, `id_token_signing_alg_values_supported: ["ES256"]`, `claims_supported`.
- Builds the JWKS from `GetPublicKey` for each configured key at startup and on a refresh interval; stateless, no database, horizontally scalable.

### Storage

- `relying_parties`: name, audience (unique), audience immutable, name editable, token lifetime in whole minutes (default 5, check 1 to 60).
- `identity_grants`: `relying_party_id`, `machine_id`, `application_id`, exactly one of the two subject columns set (`num_nonnulls` CHECK), FKs `ON DELETE CASCADE`, unique per subject and relying party. Current grants only; history is U2's. Its own two-variant target type in Rust, not `Scope`.
- `identity_key_observations`: `kid`, `first_served_at`.
- Removing a relying party cascades its grants; the removal and each revoked grant are logged, as are registration and edits.

### Public API

- `POST /identity-tokens`, `ServerDevice` extractor (machine role). Args: `audience`, optional `application_type`. Response: `token`, `expires_at`, `subject`.
- New `AppError` variants and `ERRORS.md` headings: Identity token not granted (403), Identity audience unknown (404), Identity subject not found (404), Identity subject not placed (409). Existing: Device has no server (412), Rate limited (429), Upstream (502).
- Per-caller limit through the existing `RateLimiter` (it covers only enrolment and backup progress today), applied first, before the live-machine check. Then audience, subject, grant, placement, signer, in that order (IDT, "Refusals").
- Additive; run `just gen-openapi && just gen-api` and commit the document and client.

### Private API and UI

- `fns/identity`: issuer and keys status, relying parties list/add/edit/remove (remove confirmation shows the grant count), grants get/set per machine and per application.
- Safety-mode grades, by SAFE's rules: reads are read-only; granting is write (an already-trusted machine obtaining credentials); revoking, registering and editing a relying party are write; removing a relying party is danger, declaring "cannot be undone" since its grants go with it.
- Settings tab "Identity provider" (`identity-provider-settings.html` mockup), after Certificate authority.
- `MachineEdit`: multi-select `Autocomplete` of relying parties for the machine and each application (`identity-grants.html` mockup), hidden when none are registered and none held, mirroring `NameManagementGrants`. The machine block and each application block show their `sub` with a copy button. Hidden as well for a pending or ungrouped machine, unless a grant is held.

### Outside this repo

- ops (Pulumi): KMS key(s), IAM, discovery server deployment and ingress for the issuer name, per instance (production, staging). Issuer config wired into public server, jobs and discovery server.
- bestool: local endpoint `GET /identity-token?audience=…&application_type=…` on loopback HTTP, restricted with the mechanism bestool already uses for Caddy, returning Canopy's JSON response unchanged and passing refusals through with their problem type. The SDK hook is `identityTokenProvider: async () => (await (await fetch(url)).json()).token`.
- Kubernetes workloads: the two breakdown entries.

## Build steps

- [ ] U2 audit log available: device actor with handler-set application, explicit no-change outcome, commit before response
- [ ] Identity actions in the `Action` enum, with retention
- [ ] Signer trait with KMS and in-process implementations; `kid` derivation; DER to JOSE signature conversion
- [ ] Instance configuration for issuer and keys
- [ ] Migrations: relying parties, identity grants, key observations (`just migration`)
- [ ] Database models and tests for grants (exactly-one subject, cascade, uniqueness) and relying parties (unique audience, lifetime bounds)
- [ ] Discovery server crate and image
- [ ] Jobs: served-JWKS observation, signing-key selection, `idp-discovery` self-alert
- [ ] Public server `POST /identity-tokens`: subject resolution, refusals, claims and `sub`, alphabet guard, rate limit, issuance entry, not-granted entry committed ahead and settled failed
- [ ] Error variants and `ERRORS.md`
- [ ] `just gen-openapi && just gen-api`
- [ ] Private server fns for settings and grants; `just gen-openapi`
- [ ] Settings tab and machine edit grants UI (grants calls only on change), with Playwright coverage
- [ ] ops changes (separate repo)
- [ ] bestool local endpoint (separate repo)
