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
- **History:** pgro's commit history is kept, imported into the Canopy repo so blame and old references keep resolving.
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

## Alternatives to the narrow tailnet client

The tailnet sidecar stays whichever of these is chosen: pgro reads the tailnet's MagicDNS suffix from it to build the URL of a replica exposed on the tailnet.
None of the alternatives removes it, so none gains from that.

### Kubernetes OIDC

pgro presents its projected service-account token, and Canopy verifies it against the cluster's issuer.
Canopy's identity provider (IDP, IDT) runs the other way: it issues tokens for external services to verify, and verifies none.
This would be a third kind of device identity beside a key and a tailnet node, with per-cluster trust (issuer, keys, audience) and a mapping from a service-account subject to a device.

- For: no long-lived secret and no tailnet identity to attach; the relay and anything else in a cluster could use it too.
- Against: a new authentication path in the device-auth extractor, with its own spec and security review, to build before the move could use it.
- It is independent of where pgro's source lives.
  The transport is one file behind `CanopyTransport`, so it can be swapped later without redoing the move.

### What the relay does

The relay authenticates with a device key in a QUIC client certificate, with proof of possession and Canopy's key pinned by the relay.
That is the same key-based identity pgro already holds for its mTLS fallback, over a stronger channel than the HTTP path, where TLS ends at an ingress and authentication rests on a header.
The relay's protocol carries filings and named requests, never Kubernetes objects, so pgro's worklist and credential calls do not fit it without widening it.
Using the key over the public HTTP path would keep the secret and the certificate minting that tailnet-only removes.

### Moving `bestool-canopy` here, with bestool depending on it

`bestool-canopy` is the device-side half of Canopy's contract: registration, the encrypted machine store, DNS names, backup helpers, certificates and the transport.
Its three dependents in bestool are `alertd`, `bestool` and `tamanu`.
It has had 123 commits, 47 of them since August, and its recent history is full of Canopy-driven changes (a new API version, names, backup, tailnet-only default).
bestool also keeps `canopy/*` specs describing Canopy behaviour from the device side.

- For: one client for every consumer, and every contract change lands atomically for pgro, bestool and the device agent; the device-side specs sit beside the server's.
- Against: it is a much wider move than pgro's.
  It carries Windows paths, `algae-cli`, `sysinfo`, `machine-uid` and `miette` into the server's workspace, and bestool's field tooling would wait on Canopy's pipeline to change its own client.
  Canopy's workspace is all `publish = false`, so it would have to be a second standalone published project like `bes-canopy-api`, or be folded into it, and bestool's release train would need re-pointing.
  Canopy already depends on `bestool-kopia`, so each repo would depend on the other.
- Staging loses little: pgro's narrow transport implements the same `CanopyTransport` trait, so if the client moves later pgro swaps it for the shared one.
  The cost is a small duplicate in the meantime.
- What decides it is how much of `bestool-canopy`'s churn follows Canopy's contract and how much is device-local; that split has not been measured.

## Frictions to resolve when moving

- `k8s-openapi`: pgro enables `latest`, Canopy pins `v1_32`, and only one version feature may be enabled.
- pgro's integration tests run in a kind cluster with MinIO; they need a path-filtered CI job so they do not run on every Canopy change.
- pgro's own AGENTS.md conventions (conventional commits, winnow for parsers, `#[expect]`) need reconciling with Canopy's.
- pgro's three specs (`canopy/intents`, `operator/placement`, `replica/users`) fold into Canopy's spec tree; the intents spec merges with RST.
- Canopy's per-commit deploy needs a stack path for pgro and a way to keep operator-only changes from redeploying Canopy, and the reverse.
- The `ghcr.io/beyondessential/postgres-restore-operator` image name is referenced from infra.
- Open pgro branches and dependabot PRs, and what happens to the old repo.

## Can pgro talk over the tailnet today

Yes, in code, on both sides, with no further work.

- **Server:** the whole device API, restore routes included, is nested under the private server's `/public` mount, which is the one surface that welcomes tagged tailnet nodes (pgro's sidecar is tagged `tag:pgro-operator`).
  The restore spec (RST) already says either transport satisfies the role.
- **Client:** `bestool-canopy` probes the tailnet endpoint first and prefers it when reachable; pgro routes `*.ts.net` through the sidecar's SOCKS5 proxy; and the infra stack enables the sidecar whenever `tailscaleAuthKey` is set.
  So an operator deployed with the sidecar is already on the tailnet path.

What the repository cannot show is operator state: whether the deployed pgro stack sets `tailscaleAuthKey`, and whether its tailnet node is attached to the device holding the restore role.
That decides whether this is already how pgro talks to Canopy.
It matters because an unrecognised tailnet node is refused as unauthenticated, not with the tag refusal that triggers the mTLS fallback, so a reachable tailnet with an unattached node fails outright rather than falling back.

- [ ] Before the tailnet-only operator deploys, confirm the pgro stack sets `tailscaleAuthKey`, and that the pgro node is attached to its Canopy device (and carries `TAILSCALE_REQUIRED_TAG` if the server sets one).
  If it already works over the tailnet today, there is nothing to do.

## Open

- Whether `bestool-canopy` relocation and Kubernetes OIDC are follow-up decisions that leave this move as it is, or change its shape.

The card ends at this evaluation plus an ordered build plan for the move itself; specs wait for the card that does the move.
