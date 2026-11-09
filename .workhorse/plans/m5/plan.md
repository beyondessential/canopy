# Move pgro into the Canopy repo

pgro (`beyondessential/postgres-restore-operator`) is a Kubernetes operator that restores Kopia-backed Postgres snapshots into replicas.
Canopy is its only driver in practice, and the restore contract between them is specified on both sides and evolves together.
The evaluation came out in favour of moving it, on the terms below.

## Decisions

- **History:** pgro's commit history is kept, imported into the Canopy repo so blame and old references keep resolving.
- **Deploy:** pgro auto-deploys on every merge to `main`, through Canopy's deploy mechanism with its own stack in the infra repo.
- **Standalone mode:** removed in the move.
  The hand-written, Secret-based path (`kopiaSecretRef`, `snapshotFilter`, running with no Canopy configured) goes; the replica and restore CRDs stay, because Canopy-managed replicas are materialised through the same controllers.
- **Canopy client:** `bestool-canopy` stays out of this repo.
  pgro gets a small client of its own on the in-repo `bes-canopy-api`, narrowed to what it uses.
- **Transport:** tailnet only.
  The mTLS fallback, the device-key secret and the `canopyBaseUrl` setting are dropped.
  An operator whose Tailscale sidecar is down loses its link to Canopy until the sidecar returns; that has happened once in two years of running these clusters, which is acceptable.

## Build plan

Ordered so the history import is a pure move, and each later step is its own commit.

### Before anything deploys

- [ ] Confirm the pgro stack sets `tailscaleAuthKey` and that the pgro tailnet node is attached to its Canopy device, which holds the restore role (and carries `TAILSCALE_REQUIRED_TAG` if the server sets one).
  An unrecognised tailnet node is refused as plain unauthenticated, not with the tag refusal that triggers the mTLS fallback, so a reachable tailnet with an unattached node fails outright.
  If pgro already works over the tailnet today, there is nothing to change.
- [ ] In each cluster, list `PostgresPhysicalReplica` objects that have no `canopySource` and delete the ones no longer wanted, since removing standalone mode removes the fields they depend on from the CRD.
- [ ] Confirm the operator pod only schedules on arm64 nodes, or give it a selector that says so.
  Canopy's CD builds arm64 only; the `db-replica` pool is arm64, but pgro's own CD builds both architectures and the workload tier has x86 pools.

### Import

- [ ] Rewrite pgro's history under `crates/pgro/` and merge it into Canopy as unrelated history, placing its `.workhorse/specs/` and `.agents`/`.claude` content where Canopy keeps them rather than under the crate.
- [ ] Add `crates/pgro` to the workspace and get `cargo check` passing with nothing else changed.
  Bring the `k8s-openapi` entry to one version feature, with the `schemars` feature pgro's CRD derive needs: pgro enables `latest`, Canopy pins `v1_32`, and only one version feature may be on.
  Confirm the pinned version against the clusters' control-plane version, as Canopy's manifest already asks.
- [ ] Align the shared dependencies (`kube` with `derive` and `ws`, `reqwest` with `socks`, and the rest) on the workspace entries.

### Client

- [ ] Replace `bestool-canopy` with `bes-canopy-api` by path, and write the transport as one file implementing `CanopyTransport` over `reqwest`.
  It sends `*.ts.net` through the sidecar's SOCKS5 proxy, addresses the tailnet endpoint under the `/public` mount, sets a user agent, refuses redirects and returns the response untouched.
- [ ] Make the tailnet URL operator configuration; it is hardcoded in `bestool-canopy` today.
- [ ] Update `canopy-proxy` and the schema-build registration call to the same types.
- [ ] Check that cargo accepts a path dependency on `crates/canopy-api`, which is its own workspace, and that `just check-generated` and the semver check still pass.
- [ ] Test the transport against a local server: the `/public` prefix, the proxy rule, no redirects, and a refused tailnet identity surfacing as an error.

### Remove standalone mode

- [ ] Make Canopy configuration required at operator startup, and a failure to build the client a startup error rather than a legacy-only mode.
- [ ] Remove the Secret-based Kopia path from the replica controller and CRD types (`kopiaSecretRef`, `snapshotFilter` and what only they use), regenerate `crds.yaml`, and update the README's CRD tables.
- [ ] Drop the README quick start and standalone install steps, and the integration cases that exist only for that path (invalid and missing Secret, Secret-backed lifecycle); keep the Canopy-driven integration tests.

### Specs and conventions

- [x] Draft the operator's specs under `.workhorse/specs/operator/` in Canopy's prose style: an overview (PGRO) covering the Canopy-only, tailnet-only operator, and pgro's intents (INT), placement (PLACE) and replica-user (USER) specs converted from their checklists.
  They sit in an area of their own rather than inside RST, which describes Canopy's side of the contract generically and puts a consumer's own behaviour out of scope; RST links to them.
  Their ids do not collide with Canopy's.
- [x] Remove pgro from the repositories the vulnerabilities spec lists.
- [ ] Remove pgro from the repositories the vulnerability tracker's source table lists, since its dependencies are now in Canopy's own lockfile.
- [ ] Delete pgro's `.workhorse/specs/` from the imported tree once the operator specs are the only copy.
- [ ] Merge pgro's AGENTS.md into Canopy's as a section for `crates/pgro`, so its rules do not read as applying to the whole workspace.
  Carry: its lessons (TCP keepalives to detect dead Postgres connections, no wall-clock timeouts on long-tailed operations, keepalives on both the operator's sockets and the migration Job's URI); do the work in the operator rather than in a script that runs in a Job; add integration cases to the kind-cluster workflow, tell the user they only run in CI, and keep the README's CRD tables in step with CRD changes; the parser rule (winnow unless trivial), the import grouping, `use` before `mod`, the `foo.rs` and `foo/sub.rs` module layout, and `#[expect]` with a reason instead of `#[allow]`.
  Drop: the rule against changing `bestool-canopy`'s build behaviour, since the operator no longer depends on it; `cargo test` and `cargo clippy` instructions, which Canopy's `just` recipes replace; and the agent-environment rules about `git --no-pager`, working from a branch and interactive commands.
  Keep Canopy's own rules where the two overlap (summaries, comments, conventional commits).
- [ ] Import the `pgro-status` skill as `.agents/skills/pgro-status/`, which `.claude/skills` already points at, and update it for the new shape: drop the legacy `RestoreSchedulingSuspended` row and the cron-schedule wording, since every replica is Canopy-driven and its cadence comes from its intent's parameters; and add a step that checks the operator's link to Canopy, since the operator now has no path to Canopy but the tailnet sidecar.
  Keep its rule against putting site, customer or real data figures in commits and PRs, which Canopy's own anonymity rule matches.
- [ ] Reconcile the repository tooling pgro brings with Canopy's: pgro's dependabot cargo group folds into Canopy's (the crate is a workspace member, so no new directory), its review-hero `project` and `ignore_patterns` fold into Canopy's single config, and pgro's pull request template, editorconfig, rustfmt, typos and taplo settings give way to Canopy's where Canopy has them and are kept only where it has none.
  Drop pgro's `release.toml`, `Containerfile`, `.node-version` (if only the retired workflows used it) and release and CD workflows with the old repository.
- [ ] Add the traceability comments (`// spec: PGRO`, `INT`, `PLACE`, `USER`) where the operator code implements them, and the Playwright or integration coverage the specs call for, when the code moves.

### CI and deploy

- [ ] CI: pgro's unit tests and clippy run under Canopy's existing jobs.
  Add the CRD generation and manifest lint as a check of generated files.
  Move the kind-cluster integration workflow over, filtered to changes under `crates/pgro/` and `crates/canopy-api/`, so it does not run on every Canopy change.
- [ ] CD: add the operator and `canopy-proxy` to a build group so they ship in Canopy's single image, and add a deploy job for the pgro stack beside the existing one, with its own stack path variable.
  Every merge then restarts the operator, so confirm its reconcile is restart-safe; if that proves noisy, filter the pgro deploy by path.
- [ ] Infra: point `pulumi/data/pgro` at the Canopy image with the operator as its command, remove the device-key secret and `canopyBaseUrl`, add the tailnet URL setting, and stop bumping `imageTag` by hand.

### Cutover

- [ ] Deploy and watch one full restore cycle on a staging cluster, then the rest.
- [ ] Carry across or close pgro's open branches and dependabot PRs, archive the old repository with a pointer to the new location, and retire its release workflow and tags.

## Evaluation

### Where the coupling was

- Canopy decides what to restore (intents, worklist, credentials, verification); pgro executes.
  The contract lived in Canopy's managed restore replicas spec (RST) and again in pgro's own `canopy/intents.md`.
- pgro reached Canopy through `bestool-canopy`, which pins the published `bes-canopy-api`, so a restore-API change travelled Canopy, `bes-canopy-api`, `bestool-canopy`, pgro: three releases.
- pgro was tag-released and its `imageTag` in infra's `pulumi/data/pgro` was bumped by hand; Canopy deploys every push to `main`.
- Canopy's workspace already carries `kube`, `k8s-openapi` and `bestool-kopia`, and its `relay` crate is a precedent for a cluster-resident component.

### Can pgro talk over the tailnet today

Yes, in code, on both sides.

- **Server:** the whole device API, restore routes included, is nested under the private server's `/public` mount, the one surface that welcomes tagged tailnet nodes (pgro's sidecar is tagged `tag:pgro-operator`).
  The restore spec (RST) already says either transport satisfies the role.
- **Client:** `bestool-canopy` probes the tailnet endpoint first and prefers it when reachable; pgro routes `*.ts.net` through the sidecar's SOCKS5 proxy; and the infra stack enables the sidecar whenever `tailscaleAuthKey` is set.

What the repository cannot show is operator state, which is why the first item of the build plan is a check.

### The narrow client

`bes-canopy-api` depends on no HTTP client: a consumer supplies a `CanopyTransport`.
pgro calls six operations (capabilities, worklist, credentials, backup progress, restore verification, and reporting-schema registration through a raw transport call), so it needs one transport over `reqwest` and a SOCKS5 proxy.
`bestool-canopy` is about 4,100 lines, most of it for things pgro never does: certificate minting and renewal, registration, the machine store, naming, tailnet discovery with DNS and hardcoded-address fallbacks.

For:

- A restore contract change lands in one PR, with pgro on the in-repo API crate by path.
- Drops `bestool-canopy` and what it pulls in (`miette`, `algae-cli`, `hickory-resolver`, `rcgen`, `machine-uid`, `sysinfo`).
- Removes the device-key secret and the `canopyBaseUrl` setting from the pgro stack.

Against:

- Losing the mTLS fallback (accepted above), the tailnet probe, and the hardcoded-address fallback; the proxy resolves names, so the last should not matter, but it is untested.
- A second implementation of a transport bestool already has.

### Alternatives considered and not pursued

The Tailscale sidecar stays whichever is chosen: pgro reads the tailnet's MagicDNS suffix from it to build the URL of a replica exposed on the tailnet.

- **Kubernetes OIDC.**
  pgro presents its projected service-account token and Canopy verifies it against the cluster's issuer.
  Canopy's identity provider (IDP, IDT) runs the other way, issuing tokens and verifying none, so this is a new device identity kind with per-cluster trust and a subject-to-device mapping, with its own spec and security review.
  It would remove the long-lived secret and the tailnet identity to attach, and the relay could use it too.
  It is independent of where pgro's source lives.
- **What the relay does.**
  The relay authenticates with a device key in a QUIC client certificate, with proof of possession and Canopy's key pinned.
  That is the identity pgro already holds for its fallback, over a stronger channel than the HTTP path, but the relay's protocol carries filings and named requests, never Kubernetes objects, so pgro's calls do not fit it without widening it.
  The key over the public HTTP path would keep the secret and the certificate minting that tailnet-only removes.
- **Moving `bestool-canopy` here, with bestool depending on it.**
  It is the device-side half of Canopy's contract, with three dependents in bestool (`alertd`, `bestool`, `tamanu`) and 47 of its 123 commits since August.
  One client would serve every consumer and the device-side specs would sit beside the server's.
  But it carries Windows paths, `algae-cli`, `sysinfo`, `machine-uid` and `miette` into the server's workspace, makes bestool's field tooling wait on Canopy's pipeline, needs a second standalone published project because Canopy's workspace is all `publish = false`, and leaves each repo depending on the other since Canopy uses `bestool-kopia`.
  How much of its churn follows Canopy's contract has not been measured.
