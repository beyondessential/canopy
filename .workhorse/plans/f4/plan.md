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

- Declaring is routing only: it ties a DNS name to an application and publishes nothing. Address records exist only where an application registers addresses, so DNS managed outside Canopy is left alone.
- A request on a machine hosting several applications resolves without an operator where it can: a declaration first, then the application type the request names, then the one application holding the needed grant whose group covers the DNS name. A request that resolves declares the DNS name (as certificate requests already do), so renewals and later requests follow the declaration. Applies to address registrations as well as certificate requests.
- The application's URL is not used to resolve; it stays presentation only (FLT).
- The certificate request and address registration bodies gain an optional application type. Optional and additive, so no public API break; run `just gen-openapi && just gen-api` and commit both.
- A request naming a type other than the declaring application's on the same machine is refused, naming the declaring type.
- Where both applications hold the grant and the agent names no type (the case that started this card), the request is refused as undeclared and recorded, and an operator declares; the operator-side UI below stays for exactly that.
- Declaring and releasing is offered on the application page (in Names and certificates) and on the machine page, which shows every application's DNS names together.
- Canopy records DNS names a machine asked about that no application on it declares, and the machine page presents them as not yet declared, with a control to declare each on one of the machine's applications. Recording must not distinguish a DNS name declared on another machine from one declared nowhere, for the same reason the refusal does not.
- Undeclared requests surface as a notice, not a check: on the group page while any of its machines has one, and one fleet-wide notice on the Status page naming the groups. No incident, no notification channel.
- An operator disposes of an undeclared request by declaring it or denying it. A denial is per machine and DNS name, covers address and certificate requests, records who, when and an optional note, and is checked before resolution so grants can't resolve past it. Denied requests are refused as `dns-name-denied` (403, own ERRORS.md entry) and are not recorded, so they never re-raise the notice. A denial stands until lifted or until the DNS name is declared on one of the machine's applications.
- A request naming an application type the machine contradicts is refused as `dns-name-type-mismatch` (409, own ERRORS.md entry): a type other than the declaring application's, or one no application on the machine is, including on a single-application machine. Following it would declare the DNS name for the wrong workload.
- A DNS name declared on another machine narrows exactly as an undeclared one, so it meets the grant and domain checks like any other and is refused as undeclared only at the declaration. Outside the group's domains both read `name-not-entitled`, so no machine can probe which DNS names the fleet declares. Recording and clearing the undeclared record therefore settle on the request's final outcome, not inside `authorise`.
- A denial's note is for operators: the device-facing refusal names the DNS name only, since the machine is the lower-trust side and the note now reaches agent logs through `CanopyHttpError`.
- Lapsed undeclared records are pruned by the monitor sweep; recording prunes only the asking machine's own and holds a lock on its machine row, so concurrent refusals cannot overshoot the bound.
- `CanopyHttpError` includes the problem document's detail in its message when the body is one.
- An operator-declared DNS name with no addresses presents as declared, not as withdrawn or published.
- An undeclared ask is recorded per machine and DNS name, with when it was last asked and whether for an address or a certificate. It goes when an application on the machine declares the DNS name, when the machine's next request about it is accepted, or after a day without the machine asking about it, so a site removed from Caddy drops off without anyone acting. Any machine can have one: a single-application machine is refused as undeclared for a DNS name another application holds.
- Declaring a DNS name outside every domain the application's group controls is allowed, and it presents with a warning that nothing can be published or certified for it until the group controls a covering domain. The device path still refuses it at step 5.
- The device-facing refusal for a DNS name no application on the machine declares gets its own problem type, `dns-name-undeclared` (403, with an ERRORS.md entry under the matching heading), implementing CRT's existing rule that each authorisation check is reported distinctly (today step 2 and step 4 both answer `name-not-entitled`). It reads the same whether the DNS name is declared on another machine or nowhere. The public API document gains it as a documented refusal; a new problem type is additive, so no compatibility break. A bestool follow-up card (bestool Q3) has it skip rather than fail on that refusal.

## Build

- [x] Errors: `DnsNameUndeclared` and `DnsNameDenied` (403, problem types `dns-name-undeclared` / `dns-name-denied`), ERRORS.md entries
- [x] Migration: `undeclared_dns_names` (machine, DNS name, what it was asked for, first/last asked) and `denied_dns_names` (machine, DNS name, who, when, note), both unique per machine and DNS name, cascading on machine delete
- [x] Database models: record/clear/list undeclared requests (reads ignore ones older than a day; recording prunes them), deny/lift/list denials; declaring a DNS name for an application clears both for the application's machine
- [x] `authorise` rewrite: denial first, then resolution (declaration, named type, grant + covering domain), refusing and recording undeclared; type mismatch against a declaration refused naming the declaring type; accepted request clears its record
- [x] Public API: optional `application_type` on `RequestCertificateArgs` and `RegisterNameArgs`; documented refusals; `just gen-openapi && just gen-api`
- [x] `register` / `request` race refusals answer `dns-name-undeclared`
- [x] Public-server tests for each resolution path, denial, recording and clearing
- [x] canopy-api: `CanopyHttpError` message includes the problem detail
- [x] Private API: `certificates/for_machine` (declared DNS names across the machine's applications, undeclared requests, denials), `certificates/deny`, `certificates/lift_denial`, `certificates/undeclared_notices` (optional group filter); `NameView` says whether the DNS name lies within the group's domains
- [x] Private-server tests
- [x] SPA application page: DNS names relabel, Declare field, Release per row, declared-without-addresses and out-of-domain states
- [x] SPA machine page: DNS names section with undeclared requests (declare to an application, deny with note), declared DNS names when several applications, denials with Lift
- [x] SPA notices on the group page and the Status page
- [x] Playwright coverage for the above, seed helpers for undeclared requests and denials
- [x] Test cases file
- [x] `just check`, `cargo fmt`, `just typecheck`, tests
