# Testable administrative identity

Scenarios verifying the debug-only opt-in that switches the auth extractors from the fixed development identity to the real `Tailscale-User-*` header path, so administrative status can be denied and granted through the real allow-list.

## Opt-in resolution

- [x] With `CANOPY_TRUST_TAILSCALE_HEADERS` unset, a debug build acts as the fixed `admin@localhost` administrator (existing integration suite stays green, unchanged)
- [x] A non-blank value opts into trusting real headers; a present-but-blank value counts as unset (unit test)

## Real path, opt-in set

- [x] A request with no identity headers is a definite non-admin: the probe reports `false` and an admin endpoint is refused (verifies spec: ADM)
- [x] A login absent from the allow-list is authenticated but denied admin (verifies spec: ADM)
- [x] A login on the allow-list resolves to admin and reaches gated endpoints (verifies spec: ADM)
- [x] Different logins on the same running stack resolve independently per request

## Guarantees not covered by automated tests

- [ ] A release build ignores `CANOPY_TRUST_TAILSCALE_HEADERS` entirely and always authenticates (compile-time guarantee via `cfg!(debug_assertions)`; verify by inspection of a release build)
- [ ] End-to-end: a Playwright spec drives an admin denial through the real path rather than intercepting the browser probe (enabled by this card; belongs to the permission-tier work that builds on it)
