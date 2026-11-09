# Evaluate moving pgro into the Canopy repo

pgro (`beyondessential/postgres-restore-operator`) is a Kubernetes operator that restores Kopia-backed Postgres snapshots into replicas.
Canopy is its only driver in practice, and the restore contract between them is specified on both sides and evolves together.

## Where the coupling is today

- Canopy decides what to restore (intents, worklist, credentials, verification); pgro executes.
  The contract lives in Canopy's managed restore replicas spec (RST) and again in pgro's own `canopy/intents.md`.
- pgro reaches Canopy through `bestool-canopy`, which pins the published `bes-canopy-api`.
  A restore-API change therefore travels Canopy, `bes-canopy-api`, `bestool-canopy`, pgro: three releases.
- Canopy's workspace already carries `kube`, `k8s-openapi` and `bestool-kopia`, and its `relay` crate is a precedent for a cluster-resident component.
- Canopy's CD already builds several images from a list of binaries and deploys by setting `imageTag` on a Pulumi stack in the infra repo.
  pgro is tag-released and its `imageTag` in infra's `pulumi/data/pgro` is bumped by hand.

## Decisions so far

- **Deploy:** pgro auto-deploys on every merge to `main`, through Canopy's deploy mechanism with its own stack path.
- **Standalone mode:** removed in the move.
  The hand-written, Secret-based path (`kopiaSecretRef`, `snapshotFilter`, running with no Canopy configured) goes; the replica and restore CRDs stay, because Canopy-managed replicas are materialised through the same controllers.
- **Canopy client:** pgro does not carry `bestool-canopy` into the repo.
  It gets a small client of its own, narrowed to what it uses.
- **Transport:** tailnet only.
  The mTLS fallback, the device-key secret and the `canopyBaseUrl` setting are dropped.
  An operator whose Tailscale sidecar is down loses its link to Canopy until the sidecar returns; sidecar downtime has been rare enough in practice to accept.

## The narrow client

`bes-canopy-api` depends on no HTTP client: a consumer supplies a `CanopyTransport`.
pgro calls six operations (capabilities, worklist, credentials, backup progress, restore verification, and reporting-schema registration through a raw transport call), so what it needs is one transport over `reqwest` and a SOCKS5 proxy to the tailnet endpoint.
`bestool-canopy` is about 4,100 lines, most of it for things pgro never does: certificate minting and renewal, registration, the machine store, naming, tailnet discovery with DNS and hardcoded-address fallbacks.

For:

- A restore contract change lands in one PR, with pgro on the in-repo API crate by path and no published crate in between.
- Drops `bestool-canopy` and what it pulls in (`miette`, `algae-cli`, `hickory-resolver`, `rcgen`, `machine-uid`, `sysinfo`), which Canopy is otherwise moving away from.
- Removes the device-key secret and the `canopyBaseUrl` setting from the pgro stack.
- Tailnet identity is already an accepted transport for the restore role (RST), so nothing changes on the server.

Against:

- With mTLS gone, an operator whose Tailscale sidecar is down cannot reach Canopy at all: no worklist, no credentials, no verification reports.
  pgro's current code keeps the mTLS path reachable without the sidecar on purpose; accepted (see Decisions).
- The tailnet endpoint is hardcoded in `bestool-canopy`; it becomes operator configuration.
- The tailnet probe and the hardcoded-address fallback disappear; with the SOCKS5 proxy resolving names this should not matter, but it is untested.
- The transport is a second implementation of something bestool already has.

## Frictions to resolve when moving

- `k8s-openapi`: pgro enables `latest`, Canopy pins `v1_32`, and only one version feature may be enabled.
- pgro's integration tests run in a kind cluster with MinIO; they need a path-filtered CI job so they do not run on every Canopy change.
- pgro's own AGENTS.md conventions (conventional commits, winnow for parsers, `#[expect]`) need reconciling with Canopy's.
- pgro's three specs (`canopy/intents`, `operator/placement`, `replica/users`) fold into Canopy's spec tree; the intents spec merges with RST.
- Canopy's per-commit deploy needs a stack path for pgro and a way to keep operator-only changes from redeploying Canopy, and the reverse.
- The `ghcr.io/beyondessential/postgres-restore-operator` image name is referenced from infra.
- Open pgro branches and dependabot PRs, and what happens to the old repo.

## Cutover

- [ ] An operator attaches the pgro sidecar's tailnet identity to pgro's existing Canopy device.
  Canopy authenticates a tailnet node only once it is attached to a device record, and an identity can be held by one device; if `TAILSCALE_REQUIRED_TAG` is set on the server, the node must also carry that tag.
  This is a precondition to deploying a tailnet-only operator, and it is operator state, not something the repository records.

## Open

- Keep pgro's commit history in the merge, or import fresh and link back.
