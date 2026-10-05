# Investigate bestool 403 errors for certificate requests

## Diagnosis

The reporting machine hosts two applications (Tamanu and mSupply), and neither `site.example.tamanu.app` nor `msupply.site.example.tamanu.app` is declared.
`authorise` step 2 (`crates/public-server/src/names.rs`) refuses an undeclared DNS name on a multi-application machine with `name-not-entitled` (403), by design.
bestool asks anyway because it acts on the union of every application's entitlements (its NAM spec, "Machines hosting several applications").

Three things compound it:

- The declare/release endpoints exist (`crates/private-server/src/fns/certificates.rs`) but the SPA has no control for them, so an operator cannot fix it without calling the API by hand. CRT "Presentation" already requires it.
- `CanopyHttpError`'s `Display` (`crates/canopy-api/src/error.rs`) prints only status and path, so the problem detail saying *why* is dropped by every consumer that reports the error.
- Nothing tells an operator a machine is asking for undeclared DNS names; they learn of it from bestool's certificate check failing.

Unblock today: `POST /api/certificates/declare` with each application's id and DNS name, as an admin in write mode.

## Terminology

What an application serves at is a **DNS name**, never a bare "name": machines, applications, groups and checks all have names, an application also has a public name, a machine reports a hostname, and a group claims domains.
CRT, DOM, the Fleet overview, Groups and STA use "DNS name" throughout.
Operator-facing copy this card adds or touches uses it too, including relabelling "Registered names" and "public names" in Names and certificates.
Code identifiers (`ApplicationName`, `application_names`, the `names` API module and public API fields) keep their existing names; new identifiers this card introduces use `dns_name`.

## Decisions

- Declaring and releasing is offered on the application page (in Names and certificates) and on the machine page, which shows every application's DNS names together.
- Canopy records DNS names a machine asked about that no application on it declares, and the machine page presents them as not yet declared, with a control to declare each on one of the machine's applications. Recording must not distinguish a DNS name declared on another machine from one declared nowhere, for the same reason the refusal does not.
- `CanopyHttpError` includes the problem document's detail in its message when the body is one.
- An operator-declared DNS name with no addresses presents as declared, not as withdrawn or published.
- An undeclared ask is recorded per machine and DNS name, with when it was last asked and whether for an address or a certificate. It goes when an application on the machine declares the DNS name, when the machine's next request about it is accepted, or after a day without the machine asking about it, so a site removed from Caddy drops off without anyone acting. Any machine can have one: a single-application machine is refused as undeclared for a DNS name another application holds.
- Declaring a DNS name outside every domain the application's group controls is allowed, and it presents with a warning that nothing can be published or certified for it until the group controls a covering domain. The device path still refuses it at step 5.
- The device-facing refusal for a DNS name no application on the machine declares gets its own problem type, `dns-name-undeclared` (403, with an ERRORS.md entry under the matching heading), implementing CRT's existing rule that each authorisation check is reported distinctly (today step 2 and step 4 both answer `name-not-entitled`). It reads the same whether the DNS name is declared on another machine or nowhere. The public API document gains it as a documented refusal; a new problem type is additive, so no compatibility break. A bestool follow-up card (bestool Q3) has it skip rather than fail on that refusal.
