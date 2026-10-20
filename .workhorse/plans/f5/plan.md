# Import an Ansible inventory file into an environment

## Notes

- **All-or-nothing against the secret store.** Imported secrets go to the secret store, which sits outside the database transaction. Applying writes each secret through `audit.commit_ahead(…)` before the transaction commits. If a later write or the commit fails, the import has to remove the secrets it already wrote, so a failed import leaves nothing in the store that no variable row points at.
- **Parse on the server.** The preview and the apply should run the same parsing, matching, and rule checks, so neither the SPA nor a stale preview can apply something the server would not have proposed. The SPA sends the file text with the operator's host assignments, skips, and secret marks, and the server re-derives everything else.
- **The old files are the test corpus.** Check the parser against the shapes found in the ops repo's existing inventories: nested `children` under `facility`, hosts keyed by `ip:port`, and structured values carrying a secret field (an IAM key pair). Keep client names out of fixtures.

## Surface survey

The import was checked against every inventory file in the ops repo (49 files, as of its 16 October 2026 head) and the variable tables of the infra repo's Linux specs (LNXANS, LNXSRV, LNXRUN, LNXDB, LNXTLS, LNXMSP).

- **Secrets.** The secrets in use are `salt`, `elastic_agent_enrollment_token`, and `tamanu_caddy_tls_dns_iam` (secret as a whole, for its nested `secret_access_key`). The spec also names `caddy_zerossl_api_key`. The name guess flags exactly these and nothing else: `key` only appears in names nested inside a secret value.
- **Positions.** Almost every variable sits at `all` or at a host. One file sets a variable (`elastic_agent_enabled`) on a child group, and no host there overrides it. Groups nest at most three deep (`all` → `facility` → a site).
- **Host keys.** Most hosts are keyed by their tailnet name and repeat it in `tailscale_name`. Four are keyed by `ip:port` with no tailnet name, and each also sets an `ansible_port` that agrees with the key. Two are keyed by a dotted name and set an IP `ansible_host`. Eight set an `ansible_host` name beside a tailnet key. No file lists a host twice. One file puts its only host straight under `all`.
- **Ansible variables.** Only `ansible_host` (always at host level), `ansible_user`, `ansible_port`, and `ansible_timeout` appear, all of which the plugin accepts. No `ansible_host` sits at `all`, so the machine-scope rule refuses nothing in the corpus.
- **Values.** There are no vault tags, no Jinja, and no YAML 1.1 boolean spellings other than `true`/`false`. `tamanu_facility_id` is sometimes a list and `tamanu_central_ip` is sometimes `false`, and both carry over as JSON.
- **Reserved.** Forty-eight files set `canopy_intent` and seven set `tamanu_clone: true`.
- **Not an inventory.** `canopy.yml` is the plugin's source file (`plugin: canopy`), which the import refuses for having no `all` group.
- **Comments.** Five files carry comment lines with operational notes (where the SSH key is kept, which VPN to join). They are appended to the group's notes.

## Rename to Ansible

The feature is "Ansible", since in Canopy the inventory is the fleet. The spec has moved to `private-server/ansible.md` and keeps its `INV` id, so `// spec: INV` references in code stay valid. The rename covers everything except names already stored, which keep their spelling:

- audit actions' stable names (`inventory_read`, `inventory_lease_taken`, …), which are written into existing log entries
- secret-store keys (`inv-vars-g-…`, `inv-vars-<rank>-…`, `inv-vars-m-…`). Renaming them would move secrets outside a transaction for no behavioural gain
- the recovery snapshot's `inventory_variables` key, which older snapshots in the vault carry

Steps:

- [ ] Migration renaming `inventory_variables` → `ansible_variables` and `inventory_leases` → `ansible_leases`, with their constraints and indexes (`just migration rename_inventory_to_ansible`)
- [ ] Database crate: `inventory_variables.rs` → `ansible_variables.rs`, `inventory_leases.rs` → `ansible_leases.rs`, types `InventoryVariable` → `AnsibleVariable`, etc.
- [ ] Audit catalogue: variants `Inventory*` → `Ansible*`, with labels "Serve environment to Ansible", "Take/Take over/Extend/Release Ansible lease", "Set/Remove Ansible variable". Add `AnsibleInventoryImported`, "Import Ansible inventory"
- [ ] Private server: modules `fns/inventory.rs` → `fns/ansible.rs` and `fns/inventory_variables.rs` → `fns/ansible_variables.rs`, mounted at `/api/ansible/*` and `/api/ansible_variables/*`, with the openapi tag and its description, log messages, and tests
- [ ] Keep the `/api/inventory/*` paths as aliases until the plugin has moved, since the plugin in the infra repo (and its copy in ops) calls them. Then remove them in a follow-up
- [ ] `just gen-openapi`, then `safety-modes.ts` grades for the new paths
- [ ] SPA: `GroupInventorySection` → `GroupAnsibleSection`, heading "Ansible", action labels, test ids. `e2e/inventory.spec.ts` → `e2e/ansible.spec.ts`, with seed helpers
- [ ] Infra repo, in lockstep: the plugin's `/api/inventory/*` calls move to `/api/ansible/*`. Links to `private-server/inventory.md` move to `private-server/ansible.md`, and `#what-an-inventory-contains` becomes `#what-a-run-is-served` (`#run-leases` and `#reaching-a-machine` are unchanged)
- [ ] `grep -rin inventor` over `crates`, `private-web/src`, `private-web/e2e` and `.workhorse/specs` should then turn up only the stored names above, Ansible's own inventory (the import), and the unrelated backup-repository inventory
