# Canopy as OIDC identity provider: plan

Specs: [IDT](../../specs/public-server/identity-tokens.md), [IDP](../../specs/private-server/identity-provider.md), and the identity provider condition in [SELF](../../specs/private-server/self-alerts.md).
Reasoning and rejected options: `.workhorse/working-docs/a5/working-doc.md`.

Grants history, issuance and not-granted refusal records, and the key usage figures all live in the audit log ([AUD](../../specs/platform/audit-log.md)).

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

- Actions added to the `actions!` catalogue in `database::audit`: `identity_token_issued` (tier Routine, as backup credentials are), `identity_granted`, `identity_grant_revoked`, `relying_party_registered`, `relying_party_edited`, `relying_party_removed` (all `retention = Year`). Add `RecordKind::RelyingParty`.
- Issuance: records are the subject (machine or application) and the relying party; detail is `jti`, `kid`, `expires_at`, `sub`. The handler calls `audit.acting_as(device_actor(&device, application))` once `application_type` resolves.
- Issuance follows the backup handler's connection pattern: resolve and check on a plain `db.get()` connection, sign with KMS holding no transaction, then `audit.begin()` and `tx.record(...)`. The layer commits before the response is returned and turns a failed commit into an error, so no token leaves unrecorded, and no transaction is held across the KMS call.
- Not granted: `audit.commit_ahead(IdentityTokenIssued { … })` then `audit.settle_failed(&pending, Some(json!({"reason": "not_granted"})))`, then return the 403. No other path commits ahead.
- Key "last signed" and thirty-day counts use `audit::count_by_detail(conn, "identity_token_issued", "kid", …)`, which skips refusals since they carry no `kid`. Relying party "last issued" uses `audit::find` with the relying party as the record acted on, `include_routine`. The removable state reads the latest issuance per `kid`, always recent since a retired key's last token was issued within a lifetime of retirement.
- No-op saves call `audit.unchanged()`: a grants save or relying-party edit that changes nothing.
- `MachineEdit`: add a grants call to the save plan only for a subject whose set changed, as the silence calls are, so an unchanged save sends nothing for grants.
- Route declarations: `routes!(public, audit(IdentityTokenIssued): identity_tokens)`; settings and grants reads `silent`; each write declares its actions (remove declares both `RelyingPartyRemoved` and `IdentityGrantRevoked`).

### Discovery server

- New crate and image, working name `oidc-disco-server`. Serves only `/.well-known/openid-configuration` and the JWKS path.
- Discovery document: `issuer`, `jwks_uri`, `response_types_supported: ["id_token"]`, `subject_types_supported: ["public"]`, `id_token_signing_alg_values_supported: ["ES256"]`, `claims_supported`.
- Builds the JWKS from `GetPublicKey` for each configured key at startup and on a refresh interval; stateless, no database, horizontally scalable.

### Storage

- `relying_parties`: name, audience (unique), audience immutable, name editable, token lifetime in whole minutes (default 5, check 1 to 60).
- `identity_grants`: `relying_party_id`, `machine_id`, `application_id`, exactly one of the two subject columns set (`num_nonnulls` CHECK), FKs `ON DELETE CASCADE`, unique per subject and relying party. Current grants only; history is the audit log's. Its own two-variant target type in Rust, not `Scope`.
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

- [ ] Identity actions in the audit catalogue, with tier and retention, and `RecordKind::RelyingParty`
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
