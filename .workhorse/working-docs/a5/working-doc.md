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

## Mockups

- `design/mockups/a5/claim-shapes.html`: S1 and S2 worked through rules for Anthropic and AWS, and what happens on re-rank, rename, pending.
- `design/mockups/a5/identity-grants.html`: grants on the machine edit page, beside name management, as one searchable multi-select of relying parties per machine and per application, so the control stays compact however many relying parties are registered.
- `design/mockups/a5/identity-provider-settings.html`: issuer, signing keys with usage and removal readiness, relying-party registry.

## Behaviour

Sketch only.

- The discovery document and JWKS are served at the issuer name itself by a dedicated discovery server (see "Discovery server").
- An authenticated device endpoint (working name `POST /machines/self/identity-token {audience}`) returning a short-lived JWT. The device is resolved from its certificate, as for backup credentials.
- Issuance is gated by an operator grant of audience to a machine, application or group, and is refused without one. Issuance is recorded before the token is returned.
- Claims carry the machine, group, rank and application types so a relying party can scope its own rules.

## Implementation options

### Signing key custody

Decided: AWS KMS asymmetric keys (ECC_NIST_P256, ES256), non-exportable; Canopy only calls `Sign` and `GetPublicKey`.

Decided: provisioning owns the keys, set in Pulumi, so adding or removing one is a `pulumi up` that also rolls Canopy onto the new configuration. Rotation is two runs: add the new key (Canopy publishes it and switches to signing with it once matured), then remove the old one at leisure.

Operational comparison that led there:

- Common to both: one KMS `Sign` per token, so a KMS outage stops new issuance and workloads lose access once their current Anthropic token expires. CloudTrail records every `Sign`. A disabled or deleted key is an issuance outage (deletion has a cancellable 7-30 day pending window). Each Canopy instance has its own key and issuer; tests and dev use an in-process key.
- Relying parties using discovery or a JWKS URL follow rotation on their own; inline-JWKS registrations need re-pushing on every rotation. Tokens are minted on demand and exchanged at once, so a JWT only needs to verify for seconds and retiring an old key is low-risk.
- **Provisioning owns the keys.** Infra creates the key and grants `Sign`/`GetPublicKey`; Canopy reads the configured keys at startup, as with managed zones, and builds the JWKS from `GetPublicKey`. Canopy switches signing to the newest key once it has been published for a minimum interval (beyond Anthropic's 15-minute guidance), so no human timing can cause signature errors. Rotation is two ops changes and two restarts: add the new key, later remove the old one. Canopy can never destroy its own keys. No scheduled rotation: the material cannot leak, and a leaked `kms:Sign` permission is fixed in IAM, not by a new key.
- **Canopy owns the keys.** Canopy holds tag-conditioned `kms:CreateKey` and `ScheduleKeyDeletion`, bootstraps its first key and rotates on a schedule or from an admin action, with no deploy. A Canopy bug or compromise can delete its own signing keys, and the keys drift outside infrastructure state. Earns its cost only with frequent rotation or inline-JWKS relying parties.

Key usage is tracked: every issuance records the `kid` that signed it, so for each configured key Canopy knows when it was first published, when it first and last signed, and how many tokens it signed. A key that has stopped signing is safe to remove once its last signature plus the maximum token lifetime has passed, and Canopy presents that moment, so the removing Pulumi run follows Canopy's word and not a guess.

### Claim set

Relying parties differ in what they can match on.
Anthropic matches a `sub` prefix, exact top-level string claims, or CEL over everything.
AWS IAM, for a generic OIDC provider, only exposes `aud`, `sub`, `amr`, `oaud` and `email` as trust-policy conditions (provider-specific claims exist for GitHub, Google and a few others, not custom issuers). That is why GitHub Actions encodes repo, environment and ref into `sub`.

Fixed claims: `iss`, `aud` (the registry entry's audience), `iat`, `nbf`, `exp`, a fresh `jti` per token (Anthropic rejects reuse), and `kid` in the header.

Decided: S2, structured `sub`.

`sub` holds exactly four things in a fixed order: group, rank, subject kind (with the application type for an application), and the subject's ID: `group:<id>:rank:<rank>:application:<type>:<id>:` or `group:<id>:rank:<rank>:machine:<id>:`. The trailing `:` terminates every segment, the last included, so no pattern can end mid-value whichever segment it stops at; an exact match written without it matches nothing, which fails closed. Names, tags and an application's machine go only in the flat claims, so `sub` changes only when policy should notice.

Separator safety: every segment value comes from an alphabet without `:` (UUIDs, the rank set, application types, which are slugs by spec), and issuance refuses rather than escapes a value outside it. That alone makes a label word appearing as a value harmless: positions are fixed, so a pattern like `:rank:production:` can only match where it belongs. A value containing `:` would let an AWS `StringLike` `*` span segments. Patterns Canopy offers always end at a separator (`…:tamanu-central:*`), so a type sharing a prefix with another cannot match.

Issuance is refused for a subject without a rank (pending) or without a group, naming what is missing, as a missing DNS grant is refused. Ungrouped subjects should no longer arise, but the refusal stands as a rule, so no token ever carries a placeholder segment.

Options as considered:

Option S1, opaque `sub`: `machine:<id>` or `application:<id>`, stable for the subject's life. Fleet facts go in flat top-level claims (`group_id`, `rank`, `application_type`, `machine_id`, ...). Works fully for Anthropic; an AWS role can only pin individual subjects.

Option S2, structured `sub`, GitHub-style: e.g. `group:<id>:rank:production:application:tamanu-central:<id>`, plus the same flat claims. AWS `StringLike` and Anthropic prefixes can both select "every production Tamanu central in group X". `sub` changes when a subject moves group or rank, which is what policy wants: a box leaving production stops matching production rules. Rules quote IDs, so Canopy should show the pattern to paste.

Claim names are unprefixed: `iss` already scopes them to Canopy and rules are written per issuer (GitHub Actions does the same with `repository`, `ref`). They stay clear of registered or commonly interpreted names (`groups`, `roles`, `name`, `email`, `scope`), stay flat so Anthropic's `claims` matcher sees them, and use underscores so CEL reaches them as `claims.rank`. Matchable values are IDs and enum values; names ride along for readability only, since a rename must not silently break a rule.

### Kubernetes workloads

Decided: A now, B later. A and B are each split out to their own card in the breakdown.

Pods have no Canopy identity; only the cluster relay is enrolled. Options:

- A: the cluster's own OIDC issuer registered directly with Anthropic. No Canopy work; policy lives only in Anthropic rules.
- B: relay-mediated exchange. A pod presents its projected service-account token, the relay performs the TokenReview, and Canopy issues its own JWT with the namespace's group and rank. One issuer and one policy and audit surface across machines and pods. Keeps the rule that Canopy holds no credential to the cluster.

### Delivery to a box

Decided: bestool exposes a local HTTP endpoint the workload calls, and bestool calls Canopy with the device key. Each call yields a fresh token, so `jti` reuse does not arise and the workload never holds the key. Stays in this card's design. What Canopy owes it is the device endpoint above.

### Issuer hostname

Decided: the issuer is part of the Canopy instance's configuration, set in Pulumi beside the signing keys, so the production and staging Canopy instances each have their own (working example `https://idp.canopy.bes.au`). The final names are a deployment choice, not a design one.

A Canopy instance with no issuer or no signing key configured issues nothing: the identity provider settings say so, as the certificate authority page does when no authority is configured, and grant controls stay hidden. Changing a configured issuer invalidates every relying party's registration, so it is a deliberate re-registration, not a tweak.

The issuer is a dedicated name of our own (working example `https://idp.canopy.bes.au`, name to be workshopped), independent of the public server's device-facing host, since `iss` can never change without re-registering every relying party.

Decided: the issuer name is routable from the start. Relying parties register in plain discovery mode, with no `discovery_base` or `explicit_url` workarounds, and strict relying parties that check the discovery document's `issuer` against where they fetched it work as they are.
The domain is one we hold, so nobody else can ever serve discovery for it.

### Discovery server

A separate binary and image (working name `oidc-disco-server`) serves exactly two unauthenticated documents at the issuer name: `/.well-known/openid-configuration` and the JWKS it points to. The ingress routes the issuer name to it and to nothing else.

- It reads the issuer and the configured KMS keys from the same Canopy instance configuration (Pulumi), and builds the JWKS from `kms:GetPublicKey`. It holds no database credentials, no `kms:Sign`, and no device authentication, so the only internet-facing piece of the identity provider is stateless and holds nothing secret.
- It is stateless and scales horizontally. If it is down, relying parties keep their cached JWKS briefly (about a minute at Anthropic) and then new exchanges fail, so it is run like any other serving component.
- Issuance stays on the public server's device API, authenticated as every device call is (client certificate or tailnet identity).
- The public server and the discovery server roll from the same configuration but separately, so the issuing side must not trust its own view of what is published. Canopy fetches the live JWKS from the issuer name and records when each key first appears there; that observed time is what the signing switch waits on. Problems with what is served are a Canopy self-alert (see SELF), a check under the `canopy` source with no other presentation: the settings page shows nothing about it while all is well, and repeats nothing while it fails. It fails when the issuer documents cannot be fetched, when the served discovery document names a different issuer, or when the served JWKS differs from the configured keys, its detail naming the keys configured but not served and served but not configured. A new key stuck unserved therefore surfaces as this failure while signing stays on the old key. It clears when what is served matches the configuration again.

### Grant model

Generic from the start: a grant names an audience, not Anthropic specifically.

Decided: grants are their own thing, held against a machine or against an application, not the `Scope` used by check policy (no group, cluster or Canopy-wide grants).

A token is requested for one of two subjects, and each needs its own grant for the audience:

- **The machine itself.** `sub` names the machine. Needs a machine grant.
- **An application on the machine, named by type.** bestool asks for, say, a `tamanu-central` token on its own box. `sub` names that application. Needs a grant on that application.

A machine grant does not cover the applications on the box, and an application grant does not cover its machine: they are different principals to a relying party, and a box carrying two workloads should not let one stand in for the other.
Like the DNS and TLS permissions, grants are withheld by default and a missing one is refused as a missing permission, not as a silence.
Storage follows the one-nullable-FK-per-grain pattern (`machine_id`, `application_id`, exactly one set) so deletion cascades, but with its own two-variant target type rather than `Scope`.

Naming the application by type is unambiguous: the data model nominally admits two applications of one type on a box, but nothing in Canopy produces that.
An archived application or machine keeps its grants, but is excluded from authentication, so it obtains nothing; un-archiving restores what it had.
Reachability plays no part in issuance.

Whatever the grant shape, audiences are a registry of relying parties an operator adds (name, audience string, token lifetime), and grants reference an entry rather than free text. That avoids typos silently granting nothing, and lets the interface say "may obtain tokens for Anthropic".

Claims carry Canopy's facts regardless (group, rank, application type, machine), so a relying party can still route further, for example production to one Anthropic workspace and test to another. Claims come only from Canopy-derived facts, never from operator-set tags, so editing a tag can never widen what a relying party accepts.

## Open questions


## Trade-offs

- Grants by group, rank or type across the fleet (reusing `Scope` with filters) were considered and rejected: a grant is a deliberate act on one machine or application, like the DNS and TLS permissions.
- Attest-only (Canopy issues to any enrolled identity, relying parties do all policy) was rejected: the card wants the policy held in Canopy.

## Testing notes
