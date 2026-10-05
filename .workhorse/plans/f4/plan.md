# Investigate bestool 403 errors for certificate requests

## Diagnosis

The reporting machine hosts two applications (Tamanu and mSupply), and neither `site.example.tamanu.app` nor `msupply.site.example.tamanu.app` is declared.
`authorise` step 2 (`crates/public-server/src/names.rs`) refuses an undeclared name on a multi-application machine with `name-not-entitled` (403), by design.
bestool asks anyway because it acts on the union of every application's entitlements (its NAM spec, "Machines hosting several applications").

Three things compound it:

- The declare/release endpoints exist (`crates/private-server/src/fns/certificates.rs`) but the SPA has no control for them, so an operator cannot fix it without calling the API by hand. CRT "Presentation" already requires it.
- `CanopyHttpError`'s `Display` (`crates/canopy-api/src/error.rs`) prints only status and path, so the problem detail saying *why* is dropped by every consumer that reports the error.
- Nothing tells an operator a machine is asking for undeclared names; they learn of it from bestool's certificate check failing.

Unblock today: `POST /api/certificates/declare` with each application's id and name, as an admin in write mode.

## Decisions

- Declaring and releasing is offered on the application page (in Names and certificates) and on the machine page, which shows every application's names together.
- Canopy records names a machine asked about that no application on it declares, and the machine page presents them as not yet declared, with a control to declare each on one of the machine's applications. Recording must not distinguish a name declared on another machine from one declared nowhere, for the same reason the refusal does not.
- `CanopyHttpError` includes the problem document's detail in its message when the body is one.
- An operator-declared name with no addresses presents as declared, not as withdrawn or published.
- An undeclared ask is recorded per machine and name, with when it was last asked and whether for an address or a certificate. It goes when the name is declared, when the machine stops hosting several applications, or after a day without the machine asking for it, so a site removed from Caddy drops off without anyone acting.
- Declaring a name outside every domain the application's group controls is allowed, and the name presents with a warning that nothing can be published or certified for it until the group controls a covering domain. The device path still refuses it at step 5.
- The device-facing refusal for an undeclared name on a multi-application machine gets its own problem type, `name-undeclared` (403, ERRORS.md entry), reading the same whether the name is declared on another machine or nowhere. The public API document gains it as a documented refusal; a new problem type is additive, so no compatibility break. A bestool follow-up card has it skip rather than fail on that refusal.
