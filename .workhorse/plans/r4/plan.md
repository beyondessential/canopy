# DNS names and TLS certificates as separate features

## Tech notes

- Today "declared" is a row in `application_names`, the address registration table. A name declared for a certificate is a row there with empty `addresses`, and `ApplicationCertificate` request handling calls `ApplicationName::declare` (`crates/database/src/application_certificates.rs`). `application_certificates` holds the orders and chains, not the declaration, so it cannot stand in for one: a declaration exists before any order and outlives release.
- A new certificate-name declaration table (application, name) carries the TLS side. `application_names` becomes the address side only. Both declare/release paths share one check that refuses a name held by a different application in *either* table, naming the holder, so exclusivity stays fleet-wide and cross-kind (spec: NAM, "Declared DNS names"). The fleet-wide unique index on `application_names.name` stops being the exclusivity mechanism and is replaced by that cross-table check, with a database-level guarantee so two concurrent declares cannot both win.
- Resolution (`crates/public-server/src/names.rs`, the `ApplicationName::for_name` lookup) finds the holder in either table, then declares for the request's kind if the holder does not already hold it for that kind.
- Undeclared records (`dns_name_dispositions.rs`) gain `asked_for` as part of their key, so a machine asking both ways has two rows. `UndeclaredDnsName::record`/`clear` take the kind. `clear_for_declaration` takes the kind.
- Denials gain the kind in their key. A denial check in the request path passes the request's kind. Declaring for a kind clears only that kind's denial.
- Public API: `/names/register` and `/certificates/request` are already separate and keep their shapes, so `bes-canopy-api` is unaffected; `/names/entitlements` keeps its shape and stays one answer covering both kinds. Confirm with `just gen-openapi && just gen-api` producing no diff in `crates/public-server/openapi.json` and `crates/canopy-api/`.
- Private API: split the `certificates` fns module. A new `dns_names` module takes `declare`, `release`, `deny`, `lift_denial`, `for_machine` and `for_group` for the address kind; `certificates` keeps the same operations for the certificate kind. Run `just gen-openapi` and commit `private-web/openapi.json` and `private-web/src/api-types.ts`.
- Frontend: one machine section component and one group section component, parameterised by kind and fed by the matching endpoints, mounted twice ("DNS names", "TLS certificates"). The application page's "Names and certificates" panel splits the same way. `humaniseRemaining` and the DNS-names-list expiry both go: one time-left component takes the certificate's state and renders the duration (floored the same everywhere), coloured by that state, with the instant as its hover text.
- The expiry colour reuses the state the API already derives from lifetime and renewal point (`valid` / `due for renewal` / `expiring` / expired), so no new threshold is introduced in the client.

## Migration

Canopy DNS is not in use yet, so every existing declaration was made for a certificate.

- Every `application_names` row becomes a certificate declaration for the same application.
- A row carrying registered addresses also stays in `application_names`, since those addresses are address-side data that must not be lost. Rows with no addresses leave it, so `application_names` holds only address declarations afterwards.
- Existing denials become certificate denials. Existing undeclared records keep the kind they already carry.
- Check against real rows before relying on the no-addresses assumption: any row found with addresses is worth a look before it is migrated.

## Steps

- [ ] Migration (`just migration NAME`): certificate-name declaration table, kind on denials and in the key of undeclared records, data migration above
- [ ] `database`: certificate-name declare/release, cross-table exclusivity, per-kind `record`/`clear`/`clear_for_declaration`, per-kind denial lookup
- [ ] `public-server`: resolution against either holder, per-kind denial and undeclared handling; entitlements answer lists both kinds apart
- [ ] `private-server`: `dns_names` module, `certificates` module trimmed to its kind, undeclared notices say which kind
- [ ] `just gen-openapi`, `just gen-api` (expect no public diff), commit generated files
- [ ] `private-web`: kind-parameterised machine and group section components, application page split, time-left component, update `types.ts`
- [x] Update `// spec: CRT#...` comments that moved (done with the spec split: `NAM` and `ADR` anchors in code)
- [ ] Rust tests: exclusivity across kinds, per-kind undeclared records and denials, resolution, migration
- [ ] Playwright: update `certificates.spec.ts` ("79 days left" becomes the new wording), add machine and group section coverage for each kind and the urgency colour
- [ ] `just check`, `just typecheck`, targeted `just test-package`
