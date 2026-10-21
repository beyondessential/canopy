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

The feature is "Ansible", since in Canopy the inventory is the fleet. The spec has moved to the `private-server/ansible/` folder: `overview.md` keeps the `INV` id (what a run is served, reaching a machine, refusal), with `variables.md` (`ANSVAR`), `import.md` (`ANSIMP`) and `runs.md` (`ANSRUN`) beside it. The rename covers everything except names already stored, which keep their spelling:

- audit actions' stable names (`inventory_read`, `inventory_lease_taken`, …), which are written into existing log entries
- secret-store keys (`inv-vars-g-…`, `inv-vars-<rank>-…`, `inv-vars-m-…`). Renaming them would move secrets outside a transaction for no behavioural gain
- the recovery snapshot's `inventory_variables` key, which older snapshots in the vault carry

Steps:

- [x] Migration renaming `inventory_variables` → `ansible_variables` and `inventory_leases` → `ansible_leases`, with their constraints and indexes (`just migration rename_inventory_to_ansible`)
- [x] Database crate: `inventory_variables.rs` → `ansible_variables.rs`, `inventory_leases.rs` → `ansible_leases.rs`, types `InventoryVariable` → `AnsibleVariable`, etc.
- [x] Audit catalogue: variants `Inventory*` → `Ansible*`, with labels "Serve environment to Ansible", "Take/Take over/Extend/Release Ansible lease", "Set/Remove Ansible variable". The `AnsibleInventoryImported` action ("Import Ansible inventory") is added with the import itself
- [x] Private server: modules `fns/inventory.rs` → `fns/ansible.rs` and `fns/inventory_variables.rs` → `fns/ansible_variables.rs`, mounted at `/api/ansible/*` and `/api/ansible_variables/*`, with the openapi tag and its description, log messages, and tests
- [x] No aliases for the `/api/inventory/*` paths: they are the private API, which ships with the SPA, so the plugin in the infra repo moves to `/api/ansible/*` first, and the cutover is ordered by the steps below
- [x] `just gen-openapi`, then `safety-modes.ts` grades for the new paths
- [x] Retag `// spec:` comments for the split: `INV#run-leases` and `INV#work-under-way` → `ANSRUN#…`, `INV#inventory-variables` → `ANSVAR#variables`, `INV#presentation` → `ANSVAR#presentation` (variables) or `ANSRUN#presentation` (runs). A bare `INV` stays where the code serves the environment
- [x] SPA: `GroupInventorySection` → `GroupAnsibleSection`, heading "Ansible", action labels, test ids. `e2e/inventory.spec.ts` → `e2e/ansible.spec.ts`, with seed helpers
- [ ] Infra repo, ahead of the Canopy deploy: the plugin's `/api/inventory/*` calls move to `/api/ansible/*`, falling back to `/api/inventory/*` on a 404 until Canopy has shipped. Links to `private-server/inventory.md` move: `#what-an-inventory-contains` → `ansible/overview.md#what-a-run-is-served`, `#reaching-a-machine` → `ansible/overview.md#reaching-a-machine`, `#run-leases` → `ansible/runs.md#run-leases`
- [ ] Deploy the rename while no Ansible lease is open. The callback plugin takes a 404 on `extend_lease` as a refusal and stops extending, so a run in flight across the cutover would carry on unleased once its lease runs out, and another operator could take the environment under it. A run started from a checkout that predates the plugin change is refused its lease and never starts, so only a run already under way is exposed
- [ ] Drop the plugin's `/api/inventory/*` fallback once Canopy has shipped
- [ ] `grep -rin inventor` over `crates`, `private-web/src`, `private-web/e2e` and `.workhorse/specs` should then turn up only the stored names above, Ansible's own inventory (the import), and the unrelated backup-repository inventory

## UI rework (mockups: ansible-section, ansible-run)

Built: the section (variables collapsed by count, a table per scope with entry rows driven from the keyboard, name completion from the catalogue, a value form shaped by the variable's type, edit as text with Ctrl+Shift+E, add a machine, the import button and dialog), the run buttons, and the run dialog from limit and assignments through the pending command to started, hold, timeout and refusal. What the plugin does with the handed-over lease (`CANOPY_LEASE`) is the infra plugin's to build.

- The run dialog takes the lease from the browser ("Start work"), so the plugin needs to be handed it: `CANOPY_LEASE=<id>` in the command, used instead of taking a lease of its own. The lease records the playbook and intent the dialog chose, so a run started with a different command is still the run the lease was taken for.
- The lease is pending until the plugin's first read under it. That read declares maintenance over what the run acts on. The spec leaves the pending period unnamed; use 5 minutes. With no read in that time, Canopy releases the lease itself, so a closed tab or abandoned dialog doesn't hold the environment.
- The command's first `cd` names the operator's own checkout, so it is shown but left out of the copied text.

## Variable catalogue

- `ansible-variables.schema.json` beside this plan is the draft catalogue for the infra repo, at `ansible/variables.schema.json`. It covers the variable tables of LNXANS, LNXSRV, LNXRUN, LNXDB, LNXTLS and LNXMSP, the reference page's derived and role-default variables that a play reads, the accepted `ansible_*` names, `managed_by_ansible`, and the reserved `canopy_intent`/`tamanu_clone`. It leaves out variables that nothing reads (`caddy_waf_enforce`, `mergerfs_version`, the letterhead pair).
- Checked against every old inventory file, with the effective variables per host, it flags two things, both in the files rather than the schema:
  - `tamanu_cache_central_dns` (5 hosts) is read by no playbook or template in infra, so it falls through as an unknown name.
  - One facility host sets `tamanu_facility_id` to a list of two ids. The facility template renders it as `["{{ tamanu_facility_id }}"]`, which a list breaks, in ops as well as infra. Either the host is configured by hand, or the variable has to become a list in infra. The schema keeps it a string.
- The infra repo is public, so Canopy fetches `https://raw.githubusercontent.com/beyondessential/infra/main/ansible/variables.schema.json` with no credentials. It keeps the last good copy in the database, so a restart while GitHub is unreachable still has a catalogue, and refreshes it periodically.
- Validation uses a JSON Schema 2020-12 validator with format checks for `hostname`, `ipv4`, `ipv6`, `uri` and `email`, since the schema relies on them.
- Keep only a copy that parses and validates as a catalogue. A self-alert check under the `canopy` source (SELF) warns after a day without a good read, or when the published copy is invalid. With no catalogue ever read, Run… is disabled.
- When a newly read catalogue marks a stored plain variable secret, move its value into the secret store (`commit_ahead`, then clear the plain value in the transaction), one audit entry per value.
- `x-canopy.applications` only filters the add row's offers. A machine with no applications is offered everything, and the API doesn't enforce it.
- `x-canopy.scope` generalises the hard-coded `ansible_host` rule. Keep the hard-coded rule as well, since Canopy's addressing depends on it whatever the catalogue says.

## Environments are their machines

- `resolve_environment` in `fns/ansible.rs` builds an environment from the group's live applications, refusing one with no application at the rank. GRP already defines an environment by its machines' rank, so this is a bug. Rebuild it from the group's live machines through `Machine::environment_rank` / `environment_ranks`, take the applications on those machines, and refuse only a group with nothing ranked. Serve only machines with a bound device or a reported application: a ranked machine with neither has no address, so it is left out rather than refusing the environment. The SPA's environment picker (`GroupAnsibleSection`) derives ranks from applications too, and needs the same fix.
- The empty-machine assignments the dialog makes are held on the lease, as machine id → application type. A served member with no applications carries its assigned type, and the plugin groups by it as it would by an application (the infra plugin needs the change, and LNXANS's "Inventory" grouping rule needs a sentence for it).
- The preset run buttons appear when the environment holds a `tamanu-*` application. The facility default applies when it holds a `tamanu-central`.

- [x] Rebuild `resolve_environment` from the group's live machines (`Machine::environment_rank` / `environment_ranks`), taking the applications on them, and refuse only a group with nothing ranked
- [x] Serve only members with a bound device or a reported application, leaving the rest out rather than refusing the environment
- [x] Derive the SPA's environment picker from machine ranks too

## Audit

New and widened actions in the `actions!` catalogue (stable names follow the new vocabulary, since none of these exist yet):

- [x] `AnsibleLeaseTaken`/`TakenOver` detail gains `playbook`, `limit` (pattern), `limited_to` (the matched machines, as records), `tags`, `skip_tags`, and `assignments` (machine → application type). All are optional, since a plugin-taken lease knows none of them. Extra arguments are never stored on the lease, so they cannot reach an entry
- [x] `AnsibleRunStarted`, important: the first read under a lease, naming the group and lease. The reads after it stay as the routine `inventory_read`
- [x] `MaintenanceDeclared` gains an optional `lease` record, set when a run's start declares the window. Holding it is a `MaintenanceAmended`
- [x] `AnsibleLeaseReleased` detail gains `reason`: `released` (by the holder), `taken_over`, or `unstarted` (by Canopy after five minutes, actor Canopy itself)
- [x] `AnsibleLeaseExpired`, actor Canopy itself, recorded by the job that notices an expired lease. Today a lease that expires leaves no entry
- [x] `AnsibleCatalogueChanged`, actor Canopy itself, naming the infra commit the file was read at (from the GitHub contents/commits API rather than the raw URL, which carries no commit) and its digest
- [x] `AnsibleVariableMadeSecret`, actor Canopy itself, naming the variable and the catalogue version that made it secret
- [x] `AnsibleInventoryImported`, as already planned

## Run limit and tags

- The limit preview has to match the host names the plugin uses. Today the plugin computes them in `host_names` (`ansible/inventory_plugins/canopy.py` in infra): the address, cut to its first label for tailnet names; where that's empty or shared, Canopy's name slugged (`[^A-Za-z0-9_.-]+` → `-`); else the machine id. Canopy adds the refusal when names still collide. That rule moves into Canopy, which serves each member's host name, so the preview and the run cannot disagree. The plugin then uses the served name.
- The pattern is the subset of Ansible host patterns that matches names and groups: `:` or `,` separated, `*` globs, `!` exclusion, `&` intersection. A pattern Canopy can't evaluate (a `~` regex) is refused rather than previewed wrongly.
- The draft catalogue's `x-canopy.playbooks` tags came from `ansible-playbook --list-tags` against each playbook (with `always`/`never` dropped). Infra should generate that list in CI rather than keep it by hand, and fail when it's stale.

## Maintenance windows for runs

A run's window is bound to its lease and covers what the run acts on: the environment, or the members its limit matches. MNT gains a window over several machines of one environment for that, which operators can also declare by hand.

- [x] Window over several machines: one window row naming its machines in `maintenance_windows.machine_ids` (at least two, at its group and rank, no `machine_id`), so a window has one declarer and one history. A set is not its environment's window: `open_for` on the environment skips it, and the one-open-per-environment index excludes it. That a machine is named by at most one open window, alone or in a set, cannot be an index over an array, so it is checked under `lock_group` by every declaration and move that names machines
- [x] Suspension (`SuspendedTargets`, `suspends`), moves, history, lease taking (`open_over`), and the declaration dialog's grain choices learn the set. The dialog's choices list "Several machines" between the environment and its machines, from an environment, one of its machines, or a set. A set left with one machine is that machine's window. Machines a set stops covering are recorded as machine move rows, marked `carried_on` while the window carries on over the rest so its own `covered_from` stands
- [x] Run-bound windows: `expected_end` stays `NOT NULL` (deviating from "nullable"), and `lease_id` binds a window to its lease. While bound, `expected_end` is kept equal to the lease's `expires_at` (`track_lease`), so the existing sweep ends it at expiry; `end_for_lease` ends it on release; `pass_lease` amends it to the new holder; `hold` gives it an end of its own and clears `lease_id`. A window the lease is served against does not move while the lease is held, held or not (the existing `serving_lease` rule)
- [x] Notifications read "lasting the run" in the declaration and "the run's lease ending" in the ending. The operator interface shows a bound window as lasting the run wherever an expected end is shown (`WindowEnd`), and a machine presents a set naming it as its own, naming the others it covers
- [x] `for_group` (graded read-only) declares the window on a lease's first read, in the transaction that stamps the lease started and records the run starting, so a read that fails starts nothing. Its audit clause declares `AnsibleRead`, `AnsibleRunStarted` and `MaintenanceDeclared`
