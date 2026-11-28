# Sign in to Tamanu through Canopy

Canopy issues a one-minute sign-in token for a person, signed with the identity provider's keys and encrypted to the key the Tamanu's box last authenticated with, and opens the Tamanu's `/login?token=…` with it.
Specs: TSI (new), with IDP, SAFE, FLT, SELF and STA touched.
The agent's half (decrypt, verify, replay record) is seedling's and Tamanu's half is Tamanu's; neither is built here.

## Notes

- **Built on V6.** The PM level (`TailscalePm` in `commons-servers` `tailscale_auth`), the projects view (`routes/Projects.tsx`) and every operator being offered every mode are V6's, and this branch is based on V6's work.
- **JWE with `ring`.** `oidc` already depends on `ring`, which has ephemeral ECDH on P-256, SHA-256 and AES-256-GCM. `ECDH-ES` needs the Concat KDF of RFC 7518 §4.6.2 on top, a few lines over `ring::digest`. Reach for a JOSE crate only if this falls short.
- **The key the box last authenticated with** is not tracked today: `device_keys` has no use timestamp. Add one, stamped on mTLS authentication, throttled so the device API does not write on every request.
- **Rate limits are counted from the audit log**, not from public-server's in-memory limiter: count `TamanuSignInIssued` entries per actor and per application over the window (AUD already counts entries by records and detail). That survives restarts and replicas, and lets the jobs sweep see who is at a limit for the self-alert. Refusals are not recorded, so "at the limit" is a count at or over it.
- **Ordering of effects** follows IDT's issuance: sign, then record in the audited transaction, which commits before the response leaves, so a token whose entry cannot be written is never handed out. Nothing outside Canopy changes when the key service signs, so nothing is committed ahead.
- **Capabilities** are held per source beside each application's reported detail (`application_reported_detail.capabilities`), replaced by every push from that source: a split push sets the list it carries, and unified and legacy pushes, which cannot carry one, set none.
- **Dev mode** is the localStorage key `canopy:dev:tamanu-sign-in` set to `1`, read by the SPA alone. The server reports each address as offered (every condition) or possible (every condition but the declaration), and the control shows on offered addresses, or on possible ones in dev mode. Issuance checks every condition but the declaration.
- **Popup blocking.** A tab opened after an awaited request is blocked by browsers. Open the tab synchronously on click, then navigate it once the token arrives, or close it and show the error in the view.
- **Origins** come from the application's URL when its scheme is `https`, and from `https://<tailnet name>` for the tailnet address, serialised with the `url` crate's ASCII origin serialisation (RFC 6454), path dropped.

## Explicitly rejected

- Mapping the person to a Tamanu user: Tamanu signs the bearer in as its own administrator.
- Keying the token on a Tamanu-reported id such as its own `deviceId`.
- Delivering by form POST or fragment.
- Sign-in for cluster-hosted Tamanu, or for a machine with no key authenticated over mTLS.
- Pinning the issuer on the agent's side.

## Build

### Token (`crates/oidc`)

- [ ] `jwt::encode` takes the header `typ`, so identity tokens keep `JWT` and sign-in tokens carry `canopy-sign-in+jwt`
- [ ] `jwk` gains the RFC 7638 thumbprint of a P-256 public key from its SPKI
- [ ] New `jwe` module: compact JWE to a P-256 SPKI with `alg` `ECDH-ES`, `enc` `A256GCM`, `cty` `JWT`, `kid` the thumbprint, and the ephemeral key as `epk`; Concat KDF per RFC 7518 §4.6.2
- [ ] Sign-in claims type (`iss`, `iat`, `nbf` = `iat`, `exp` = +60s, `jti`, `sub` `person:<lowercase login>`, `origin`, `application_key`, `application_type`)

### Capabilities (`crates/public-server`, `crates/database`)

- [ ] Migration: `application_reported_detail.capabilities text[] NOT NULL DEFAULT '{}'`
- [ ] Parse `applications.<key>.capabilities` in split pushes: an array of non-empty strings, else 400; `ApplicationReport` documents it for the OpenAPI document
- [ ] `ReportedDetail::record_for_application` writes the list; `record` (unified, legacy) writes none
- [ ] Query: which of some applications have a capability from any source
- [ ] `just gen-openapi && just gen-api`

### Database (`crates/database`, `migrations/`)

- [ ] `just migration device_key_last_authenticated`: `device_keys.last_authenticated_at`, nullable
- [ ] Stamp it on mTLS authentication in `commons-servers` device auth, throttled; tailnet authentication leaves it alone
- [ ] Query: the active key a machine's bound identity last authenticated with over mTLS, or none
- [ ] Audit action `TamanuSignInIssued` (stable name `tamanu_sign_in_issued`), important, recording the application and its machine; detail `jti`, signing key, `expires_at` as IDT records them, the encryption key's thumbprint, origin and subject
- [ ] `identity.rs` key states and per-key figures read both `IdentityTokenIssued` and `TamanuSignInIssued`
- [ ] Sign-in eligibility for an application: Tamanu central or facility, live, `reported_key` held, machine's identity has a last-authenticated active key; returns the offerable origins (public if `https`, tailnet if the bound device has a tailnet name) and whether a source declares `tamanu-sign-in`
- [ ] Rate-limit counts per actor and per application over the window, from the audit log

### Private server (`crates/private-server`)

- [ ] `fns/tamanu_sign_in.rs`: `issue` taking `TailscalePm`, `Audit`, and `{ application_id, address: public | tailnet }`; checks eligibility and the instance's issuer, applies both limits (refused as rate limited), signs via `identity::signing`, encrypts, records, returns the sign-in URL
- [ ] Mount at `/api/tamanu_sign_in` with `routes!(read_only, audit(TamanuSignInIssued): issue)`
- [ ] Application detail and the projects view responses carry each address's sign-in availability: offered, possible (all but the declaration), or neither
- [ ] `just gen-openapi`

### Device API (`crates/public-server`)

- [ ] `GET /machines/self` gains an optional `issuer`, the instance's issuer when one is configured
- [ ] `just gen-openapi && just gen-api`; the addition is compatible under `cargo-semver-checks`

### Self-alert (`crates/jobs`, `crates/database/src/self_alerts.rs`)

- [ ] `canopy` source check for a sign-in limit reached: sweep raises it, warning, with detail naming each person and Tamanu at a limit, and recovers when none is; catalogue entry and check documentation

### Operator UI (`private-web`)

- [ ] Sign-in control, shown where offered, or where possible in dev mode: opens a tab on click, calls `issue`, navigates the tab to the returned URL, or closes it and shows the refusal or failure inline
- [ ] `ServerDetail.tsx`: control beside the application link when that address is offerable
- [ ] `Projects.tsx`: control beside each offerable address, next to its `Address` link
- [ ] `just typecheck`

## Tests

- [ ] `oidc`: JWE round trip with a test-only decrypt, Concat KDF against RFC 7518 Appendix C's vector, thumbprint against a fixed vector, header `typ` per token kind
- [ ] `database`: last-authenticated stamping and throttling; key selection ignores inactive keys and a provisioned key never used over mTLS; eligibility with a machine and application seeded under one id
- [ ] `private-server` endpoint: PM issues and the token decrypts with the box's key carrying the expected claims; an all-level caller is refused (`CANOPY_TRUST_TAILSCALE_HEADERS`); refused for no `reported_key`, no authenticated key, a cluster application, an `http` URL, no issuer configured; per-person and per-Tamanu limits; audit entry and detail
- [ ] `public-server`: `/machines/self` carries the issuer when configured
- [ ] `jobs`: the self-alert raises at a limit and recovers once none is
- [ ] Playwright: the control on an application page opens the sign-in URL in a new tab; absent when not offerable; a refusal shows inline; the control beside each offerable address in the projects view
