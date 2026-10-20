# Canopy as OIDC identity provider: test cases

## Issuance

- [x] A granted application on a ranked, grouped machine obtains a token naming it, with `application_type` in the request (verifies spec: IDT)
- [x] A granted machine obtains a token naming itself when the request carries no application type (verifies spec: IDT)
- [x] Two consecutive requests return tokens with different `jti` values (verifies spec: IDT)
- [x] The response carries the token, its expiry, and the same subject the token's `sub` holds (verifies spec: IDT)
- [x] A token's `exp` minus `iat` equals its relying party's configured lifetime (verifies spec: IDT, IDP)

## Refusals

- [x] An application without a grant for the audience is refused as not granted, 403 (verifies spec: IDT)
- [x] A machine grant does not let the machine obtain a token for its application, and an application grant does not cover its machine (verifies spec: IDP)
- [x] An audience matching no relying party is refused as unknown, 404, not as not granted (verifies spec: IDT)
- [x] An application type with nothing live on the calling machine is refused as no such subject, 404; so is an archived application's type (verifies spec: IDT)
- [x] A pending machine is refused as not placed, 409, naming the missing rank (verifies spec: IDT)
- [x] An ungrouped machine is refused as not placed, 409, naming the missing group (verifies spec: IDT)
- [x] A device not bound to a live machine is refused, 412 (verifies spec: IDT)
- [x] A machine exceeding the request limit is refused, 429 (verifies spec: IDT)
- [x] With no issuer or no signing key configured, a request fails with 502 (verifies spec: IDT, IDP)
- [x] Revoking a grant refuses the next request (verifies spec: IDP)
- [x] A pending machine with no grant is refused as not granted, not as not placed (verifies spec: IDT)
- [x] An unknown audience with a nonexistent application type is refused as unknown audience (verifies spec: IDT)
- [x] A rate-limited caller not bound to a live machine is refused as rate limited (verifies spec: IDT)
- [ ] An archived machine keeps its grants, obtains nothing while archived, and obtains tokens again once restored and re-enrolled (verifies spec: IDP)
- [ ] With no issuer or signing key configured, grant choices are hidden except where a grant is already held (verifies spec: IDP)
- [x] Removing a relying party revokes its grants and later requests naming its audience are refused as unknown (verifies spec: IDP)

## Token shape

- [x] Application `sub` is `group:<id>:rank:<rank>:application:<type>:<id>:` and machine `sub` is `group:<id>:rank:<rank>:machine:<id>:` (verifies spec: IDT)
- [x] Re-ranking a machine changes the `sub` of tokens issued for it and its applications afterwards (verifies spec: IDT)
- [x] Renaming a group changes `group_name` but not `sub` (verifies spec: IDT)
- [x] Claims carry no operator-set tags (verifies spec: IDT)
- [x] No token is issued whose `sub` would carry a value with a character outside lowercase letters, digits and hyphens (verifies spec: IDT)
- [x] The token header names a `kid` present in the served JWKS, and the signature verifies against it as ES256 (verifies spec: IDT, IDP)

## Recording

- [x] Each issuance is a routine entry acting on the subject and relying party, its actor the machine device (with the application for an application subject), its detail `jti`, `kid`, expiry and `sub`, and never the token (verifies spec: IDT)
- [x] Each refusal as not granted is an issuance entry settled failed with reason not granted, on the subject and relying party, and is presented at once rather than as pending (verifies spec: IDT)
- [x] Refusals other than not granted leave nothing in the audit log (verifies spec: IDT)
- [x] When the audit entry cannot be written, no token is returned (verifies spec: IDT)
- [x] Granting and revoking are entries acting on the subject and relying party, kept for a year (verifies spec: IDP)
- [x] Saving a machine's edit page without changing its grants or its applications' grants records no grant entry and succeeds (verifies spec: IDP)
- [x] Saving a relying party unchanged records nothing and succeeds (verifies spec: IDP)
- [x] A relying party with no issuance in the last ninety days shows none in that time (verifies spec: IDP)

## Discovery and keys

- [x] The discovery server serves the discovery document at `<issuer>/.well-known/openid-configuration` with the configured issuer, and the JWKS it names (verifies spec: IDP)
- [x] The discovery server answers nothing on any other path (verifies spec: IDP)
- [x] A newly configured key is published but not used for signing until it has been observed served long enough; the previous key signs meanwhile (verifies spec: IDP)
- [x] On a fresh instance, the first configured key signs immediately, before any maturation interval has passed (verifies spec: IDP)
- [x] Once a key has signed, an older configured key is never selected for signing again (verifies spec: IDP)
- [x] A configured key never observed served raises the self-alert as a warning naming it, while signing stays on the previous key (verifies spec: SELF, IDP)
- [x] A served key set missing the key signing raises the self-alert as a failure (verifies spec: SELF)
- [x] A served key that is not configured raises the self-alert as a warning naming it (verifies spec: SELF)
- [x] An unreachable issuer, or a discovery document naming a different issuer, raises the self-alert as a failure; it clears when served documents match the configuration again (verifies spec: SELF)
- [x] A key that stopped signing shows as removable only once its last token has expired (verifies spec: IDP)
- [x] A key older than the signing key that never signed is removable as soon as the newer key signs (verifies spec: IDP)
- [x] A key not yet served, waiting to sign, or retired is presented as published (verifies spec: IDP)

## Operator interface

- [x] Adding a relying party with an audience already registered is refused (verifies spec: IDP)
- [x] A relying party's lifetime defaults to five minutes, is whole minutes, and is refused below one minute or above an hour (verifies spec: IDP)
- [x] Editing a relying party's name and lifetime takes effect; a changed lifetime applies to the next token and not to ones already issued (verifies spec: IDP)
- [x] A relying party's audience cannot be changed after registration (verifies spec: IDP)
- [x] Removing a relying party asks for confirmation stating how many grants it revokes (verifies spec: IDP)
- [x] Registering, editing and removing a relying party are entries acting on it, an edit recording what it changed, and a removal records a revocation for each grant it revokes (verifies spec: IDP)
- [x] The register shows each relying party's audience, lifetime, grant count and last issuance (verifies spec: IDP)
- [x] The machine edit page offers a searchable choice of relying parties for the machine and for each application, and saving grants and revokes accordingly (verifies spec: IDP)
- [x] The machine's choice and each application's choice show their `sub`, copyable (verifies spec: IDP)
- [x] A pending or ungrouped machine's edit page offers no identity token choice, for the machine or its applications (verifies spec: IDP)
- [x] With no relying party registered the choice is hidden, unless the subject already holds a grant (verifies spec: IDP)
- [x] The settings page shows each key's status, first served, last signed and thirty-day count (verifies spec: IDP)
- [ ] Interoperability: a token from a staging instance is accepted by an Anthropic federation rule using the documented `sub` pattern, and the minted Anthropic token calls the Messages API
