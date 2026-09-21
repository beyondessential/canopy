# In-cluster relay and transport — tech design

The foundational card for Kubernetes monitoring: build the Canopy-authored relay that runs in each cluster and the transport it speaks to Canopy over. K1 (registry), M1 and N1 (the two check families) build on what this card lays down. Verifies spec `K8S` (`monitoring/kubernetes.md`).

This plan is the working record for the technical approach. Architecture inherited from the B1 brainstorm (`plans/b1/plan.md`, "Access to clusters") is treated as settled; this card decides the mechanics B1 deliberately left to the implementation.

## Settled coming in (from B1/H1/G2/K2)

- **One relay per cluster**, Canopy-authored, holding the cluster's RBAC on its own ServiceAccount. It dials outward to Canopy; Canopy never dials the cluster and holds no cluster credential. A registered cluster *is* a relay identity.
- **Transport is QUIC (`quinn`)**, configured onto the workspace's `aws-lc-rs` rustls provider rather than the default `ring`. (H1 had the TLS carry throwaway certificates on the reasoning that WireGuard already provides confidentiality and peer auth — **superseded**, see "Identity over QUIC": the certificate carries the relay's device key and is load-bearing.)
- **Kernel-mode Tailscale sidecar** where the tailnet is used (`TS_USERSPACE=false`, `NET_ADMIN`, TUN) — userspace mode is a TCP-only proxy that QUIC can't pass. Established practice in our infra. (H1 required this at both ends unconditionally — **relaxed**, see "Tailscale is an overlay, not the gate".)
- **Identity** is a device with a `relay` role, associated with no server. (H1 authenticated it by tailnet peer tag with SPKI pinning as optional hardening — **superseded**, see "Identity over QUIC": authentication is the device key presented in the TLS handshake, and the tailnet ACL is not Canopy's concern.)
- **What crosses the connection** (settled by K2): filings (both check families, computed relay-side), three named queries (namespace roster for L1's picker, connected-and-answering handshake for K1, embedded check-suite version for SELF's skew alert), and two commands (sleep/wake). No method returns a Kubernetes object. Filings converge on the same ingestion path a device push takes. (A sixth exchange is added here by the deployment decision — Canopy naming the version the relay should run.)
- **The relay embeds `bestool-alertd` as a library** to run the harvest against each instance's local Postgres; only results cross to Canopy.
- **Protocol versioning from the start** — the relay is a second deployable (`vN`) running against Canopy (`vM`).

## Code structure — three crates

Decided. The relay is a canopy-workspace deployable, built and released from this repo alongside the servers, so its protocol contract stays in the same tree as the Canopy side that consumes it.

- **`crates/relay`** — the in-cluster binary. Owns the heavy, relay-only dependency set: `kube` + `k8s-openapi` for the watches and the substrate checks, `bestool-alertd` as a library for the harvest, `quinn` for the client end of the transport. Its own workspace member with a `[[bin]]`, not a bin hung off an existing crate, because it releases on its own cycle.
- **`crates/relay-protocol`** — the shared wire contract: message types, framing, and version negotiation. Depended on by both ends so the two deployables cannot drift on message shape. Carries no `kube` and no `bestool-alertd`; a filing crosses as its serialised payload, not as a Kubernetes object.
- **Canopy-side connection worker** — a new bin in `crates/jobs`, alongside the other long-lived Deployment pods (`backups.rs` is the pattern). It speaks the protocol and hands filings to the ingestion path; it needs no `kube` of its own, which keeps `jobs`' existing isolation of `kube`/`k8s-openapi` intact.

### Many relays, one listener

There is one relay per registered cluster, so the Canopy-side worker is not a client dialling out but a **QUIC listener holding N concurrent inbound connections**, one per cluster. Consequences to carry through the rest of the design:

- The worker keeps a **connection registry keyed by cluster**, which is what K1's connected-and-answering query and SELF's per-cluster connectivity check both read.
- Which cluster a connection belongs to is **derived from the authenticated relay identity**, never claimed by the relay in a message. The device row for the relay is the cluster's registration, so the mapping is a lookup, not an assertion to trust.
- The worker being a singleton means its loss makes every cluster unreadable at once. That surfaces correctly through the existing per-cluster connectivity check (every instance fails), but it is worth naming as a single point of failure the design accepts.

## Protocol — a stream per exchange

Decided. QUIC streams are cheap and independently delivered, so **the stream is the correlation**: no request-ID bookkeeping, no multiplexing layer, and a cancelled request is just a reset stream. A slow namespace-roster query cannot stall a queue of filings behind it, which a single multiplexed stream would allow.

- **Filings go up as unidirectional streams**, opened by the relay: open, write, close. No response body.
- **Queries and commands are bidirectional streams**, opened by Canopy: the five exchanges K2 settled (namespace roster, connected-and-answering, embedded suite version; sleep, wake), plus the version-naming command the deployment decision adds.
- Messages are length-delimited, with the message enum living in `relay-protocol`.

### Filings are unacknowledged

Accepted deliberately. The relay gets QUIC's delivery guarantee but no application-level acknowledgement that a filing was *ingested*, so a filing Canopy accepts on the wire and then fails to ingest is lost until the next refile. **The periodic refile is the reconciliation mechanism** — it already exists to survive a missed event, a restart, or a reconnection (K2's cadence), so per-filing acks would be redundant machinery covering a window the refile already closes. Worth revisiting only if the refile interval ever grows long enough that a lost filing matters in the gap.

### Version negotiation rides on ALPN

The protocol version is carried in the **QUIC ALPN token** (`canopy-relay/1` and successors). Canopy offers the range it supports and the relay picks, so an incompatible pair fails at the TLS handshake with a clear "no application protocol" rather than connecting and then failing to parse a message. Protocol versioning is present from the first release, since the relay is a second deployable running vN against Canopy vM.

Detail that is not protocol-breaking rides in a post-handshake control message instead: the embedded check-suite version for SELF's skew alert, and the relay's build info.

## Filing handover — hoist the ingestion core, two filing shapes

K2 requires filings to converge on the ingestion path a device push takes, so parity is not re-derived on Canopy's side. Today that path is `file_health_events`, a **private function in `crates/public-server/src/statuses.rs`** reachable only through the axum handler. The relay speaks QUIC, not HTTP, and `jobs` depending on `public-server` would invert the dependency direction.

### Hoist the push ingestion into `commons-servers`

Decided. Move `file_health_events`, `collect_check_results`, `split_health_from_extra`, and the `StatusPush`/`HealthCheck` types out of `public-server` into `commons-servers`, which already reaches `issues::file_check` from `tailnet_sweeps.rs`. The HTTP handler and the relay connection worker then become two thin callers of one ingestion core.

The alternative — re-implementing the conversion in `jobs` — produces exactly the drift G2 identified as the one real risk to parity ("the crate already builds its payload through the same serialisation a pushed bestool uses; the one real risk is Canopy re-modelling the filing on its side"). The refactor touches shipped code on the status-push path, so it carries regression risk of its own and wants the existing status-push tests kept green across the move.

### Two filing message types, split by family

The two check families do not share a filing shape, and forcing them into one envelope would mean a lowest-common-denominator type that fits neither.

- **Harvest filings (`alertd` source)** are per-server, and the relay already produces the payload a pushed bestool produces. So the filing message **is the status-push body** rather than a re-modelled filing type, fed straight into the hoisted ingestion. Parity becomes structural rather than something maintained.
- **Substrate filings (`kubernetes` source)** scope to a server, to a server group (a namespace), or Canopy-wide with each cluster an instance, and the source is reserved from the device API — so there is no push analogue to converge on. These construct `CheckFiling` / `InstancedCheckFiling` and go through `issues::file_check` directly.

Both carry `Scope` from the single `database::issues::Scope` enum; the relay never hand-rolls a server/group discriminator of its own.

## Identity over QUIC — the existing device key, not a pinned throwaway cert

**This revises H1 and the card description**, which had the relay carry a throwaway certificate and be authenticated by its tailnet peer tag, with SPKI pinning as optional hardening. Canopy already has the concept those were reaching for: a device with a **device key** (spec `DPK`, provisioned credentials). Adopting it for the QUIC connection answers identity, differentiation between relays, and enrollment in one move, and adds no new mechanism.

### How it works

The relay presents a **client certificate carrying its device key** in the QUIC TLS handshake. Canopy reads the peer certificate's `subject_pki.raw`, looks it up with `Device::from_key`, and checks the resulting device carries the `relay` role. That is the same SPKI lookup the HTTP mTLS path performs (`device_auth/mtls.rs`), against the same `device_keys.key_data` column, so the two paths authenticate the same way against one store.

`keygen::generate_device_key` already mints exactly this: a P-256 keypair whose SPKI is derived *by self-signing a throwaway certificate and reading `subject_pki.raw`* — the same operation the relay's TLS stack will perform. So the bytes stored at provisioning are guaranteed to match what the QUIC handshake presents, by construction rather than by care.

### It is stronger here than on the HTTP path

Worth recording, because it inverts the usual expectation. The HTTP mTLS path carries a documented weakness (`mtls.rs`, on `ClientCertHeader`): TLS is terminated at an ingress proxy, so authentication rests on a header, there is **no proof of possession**, and a public key is not a secret — hence the whole single-trusted-header apparatus guarding against a caller pasting in an enrolled device's certificate.

Over QUIC, Canopy terminates TLS itself. The handshake *is* proof of possession: the relay must sign with the private key. No proxy, no header, nothing to spoof. The QUIC path gets for free the property the HTTP path has to work to approximate.

### Enrollment falls out of it

The DTR gap closes with no new path. An admin uses the existing **create-device workflow** with `role = relay`, Canopy mints the keypair and hands back the private key once, and that key is installed into the cluster as a Secret for the relay's deployment to read. Nothing authenticates before an operator has acted, and the relay is created, tracked, and revoked exactly as any other device is — which is what `K8S` already says of it.

Revocation is likewise the existing path: deactivate the key (`is_active`), and `Device::from_key` stops resolving it.

### Consequences

- **SPKI pinning as a separate question dissolves.** The device key *is* the pin, and it is a first-class record with a provisioning workflow behind it rather than a fingerprint captured at enrollment.
- **Relays differentiate by device key.** Each cluster's relay holds its own, so the connection maps to a cluster through the device row — the lookup the connection registry needs, and never a claim the relay makes in a message.
- **Tailnet identity is no longer load-bearing for authentication.** The tailnet still provides the network path, confidentiality, and reachability control, and its ACL remains organised in the tailnet and out of Canopy's concern. But Canopy no longer needs the tailnet directory lookup on this path at all, so no refactor of `tailnet::resolve` is required and the QUIC listener does not depend on the Tailscale control-plane API being up.

### Implementation notes

- Canopy's rustls server config must **request a client certificate and accept any** (there is no CA and no chain to validate), then Canopy inspects the presented certificate itself and does the SPKI lookup. That is a custom `ClientCertVerifier`; the verifier is deliberately not the gate, the device-key lookup is.
- `Device::add_key` refuses a second active key on a device that already carries a different one, so relay key rotation is deactivate-then-add rather than overlap-then-cut. Fine at this fleet size; note it, because it means a rotation has a window where the relay cannot reconnect.
- The relay's own verification of *Canopy's* server certificate is the remaining minor choice: skip it and rest on the tailnet for that direction, or pin. Skipping matches the original reasoning and is the default unless there is a reason to do otherwise.

## Deployment & versioning — Canopy names the version, Kubernetes rolls it

**Decided: option C below.** The relay is deployed once per cluster from `beyondessential/ops`, and thereafter Canopy keeps it current by naming the version it should be on. Canopy CI never holds credentials to a cluster.

The relay embeds `bestool-alertd`, and K2's tradeoff (a substrate check change ships as a relay release) is only acceptable because the relay redeploys on the fleet's cadence. So the deployment mechanism has to actually deliver that cadence, or K2's reasoning does not hold.

Orthogonal and worth doing under any option: **tighten Dependabot's cadence for `bestool-alertd`** specifically. The default weekly/seven-day delay is calibrated for dependencies we do not control; this one we publish and trust, so it should come in fast. A merged Dependabot PR is what triggers a release under every option below.

### Option A — two images, ops deploys

Canopy CI builds and publishes both images; `beyondessential/ops` (Pulumi, where the Tamanu k8s layout already lives) deploys the relay per cluster, pinning a tag so clusters roll independently. Optionally, only rebuild the image that actually changed.

The problem is the deploy path. Either ops rolls each cluster by hand, which will not keep the fleet on cadence, or Canopy CI gains credentials to deploy into N clusters. The latter means **CI holds a deploy path into every cluster**, the cluster inventory lives in a public repo's CI configuration (so their number and identity are exposed), and standing up a new cluster is a CI change rather than a deployment.

### Option B — harness plus pushed binary

A thin harness is deployed once per cluster and rarely changes. It holds the device key, opens the QUIC connection, receives the relay binary from Canopy, and runs it. Canopy's image carries both binaries. Versioning largely disappears: if the connected relay is not on the version Canopy holds, Canopy pushes it.

Strong on operations — no CI-to-cluster credentials, no cluster inventory in CI, adding a cluster is "deploy the harness, create the device". But it has two costs that are easy to underrate.

**It is a bespoke code-distribution and execution system.** Binary transfer, integrity verification, atomic replace, supervision, restart, crash-loop rollback, version reporting — all built here, all needing to be very reliable, because a harness that breaks has no remote path to fix it. Kubernetes already is the deployment system; the harness re-implements a Deployment and an image tag from inside a pod, and gives up image provenance, signing, `kubectl rollout undo`, and the rollout safety a real image change gets for free. It also needs payload signature verification against a key baked into the harness, or connection compromise becomes code execution — which recreates by hand the provenance an image tag supplies.

**It dissolves the boundary this card exists to build.** The compromise argument is mostly right: landing malicious code in the repo defeats A and B equally, and Canopy already holds credentials to do real damage. But the two differ in the *non-repo* case, compromise of the running Canopy process. The relay's authority is deliberately a narrow method set — no `exec`, no `portforward`, and **no method returns a Kubernetes object or a database row**. That is why a compromised Canopy in A can obtain check results but not clinical data, even though the relay itself reads every Tamanu database in the cluster. In B, a compromised Canopy ships code that the harness runs under the relay's ServiceAccount with the relay's access to every instance's Postgres. RBAC still bounds it, but RBAC was never what kept Canopy out of the databases — the method set was, and B replaces the thing that enforces it. The widening is from "check results" to "patient data in every cluster", which is a different order of thing from "Canopy can already do damage".

### Option C — Canopy names the version, Kubernetes does the rollout

Canopy tells the relay which version it should be on; the relay patches its own Deployment's image tag; Kubernetes pulls the signed image from the registry and performs the rollout. One image, one Deployment, no harness.

This keeps what B is actually after — no CI-to-cluster credentials, no cluster inventory in CI, Canopy-driven cadence, adding a cluster needs no CI change — while giving up neither provenance nor the boundary:

- **Less code than B, not more.** Patching a Deployment's image tag is a small piece of `kube` work. There is no transfer, no supervision, no rollback machinery to write.
- **Provenance survives.** Code arrives from the registry as a signed, versioned image. Canopy supplies a *version string*, never a binary.
- **Rollout safety is Kubernetes'.** A bad version that fails readiness does not complete its rollout; with `maxUnavailable: 0` the old pod keeps serving and stays connected, so Canopy can still name an earlier version. B would have to build that.
- **Compromise is bounded to published images.** A compromised Canopy can order any published relay image, including an old one with a known bug — a downgrade attack, answered with a version floor. It cannot run arbitrary code, so the method set stays the boundary.

Cost: the cluster needs registry pull access (it already has it, for its own image), and the relay needs RBAC to patch its own Deployment. That is a small widening, and the same *kind* of verb K2 already added for sleep and wake.

B's operational case is the right one and C satisfies it; where they differ, C is both less to build and the one that preserves the property this card is for.

### What C means for this card's build

- **A sixth protocol exchange.** Alongside K2's three queries and two commands, Canopy needs a way to tell a relay the version it should be on. It fits the command shape (Canopy-opened bidirectional stream), and it is the one command that is not about a Tamanu deployment.
- **Bootstrap deployment stays with ops.** `beyondessential/ops` deploys the relay per cluster once — namespace, ServiceAccount and RBAC, the Tailscale sidecar, the device-key Secret, and an initial image tag. Standing up a new cluster is that plus creating the relay device in Canopy. No CI change, and no cluster inventory in this repo.
- **RBAC gains the verbs to patch its own Deployment.** Narrow: the relay's own Deployment in its own namespace, nothing else. Same kind of verb K2 already introduced for sleep and wake, and still never `exec` or `portforward`.
- **`maxUnavailable: 0` is load-bearing, not a default to inherit.** It is what keeps the old pod serving and connected when a bad version fails readiness, which is what lets Canopy name an earlier version to recover. Set it deliberately and note why.
- **A version floor.** The lowest image tag a relay will accept being told to run, baked into the relay rather than supplied by Canopy. This is the answer to the downgrade attack C leaves open; without it a compromised Canopy could order a relay back to a known-bad release.
- **SELF's skew check changes character but stays.** It stops being "did a human roll the fleet" and becomes "did the version Canopy named actually take" — a relay stuck on an old version now means a rollout that failed or a relay that will not accept it, both of which want an operator.

## Tailscale is an overlay, not the gate

A consequence of authenticating with device keys, and a departure from H1 worth stating outright. H1 made the tailnet load-bearing: certificates were throwaway, so the ACL and the peer tag were the whole gate. With the device key in the TLS handshake, **authentication no longer depends on the tailnet at all**. Tailscale becomes what it is good at — overlay networking that reaches a cluster without exposing an ingress — plus network-level access control as defence in depth.

So the relay does not technically need Tailscale. Its Canopy endpoint is **configuration, and the transport is address-agnostic**: the same code dials a tailnet address or an ordinary one.

### Both ends must authenticate, and now the relay's side is load-bearing too

This flips the item previously left open. Skipping verification of Canopy's certificate was justified *by* the tailnet providing peer authentication in that direction; without the tailnet that justification is gone, and it was never a property to rely on lightly under option C — an endpoint a relay mistakes for Canopy can tell it which image to run. A downgrade is bounded by the version floor, but nothing bounds the rest.

**Decided: the relay verifies Canopy's identity, always, on every transport.** Symmetric with the device key: the relay's configuration carries Canopy's expected public key, pinned, alongside its own key. No CA and no chain in either direction, one verification path whether or not the tailnet is in the way, and nothing that becomes unsafe if a deployment later drops the overlay.

### Canopy's own cluster dials directly

The relay in Canopy's own cluster connects to Canopy's Service over cluster DNS, with **no Tailscale sidecar**. That removes the loopback out to the tailnet and back that was the standing cost of reading the local cluster through a relay — it is now a configuration difference (which address, sidecar or not) rather than anything in the code, and the single code path survives intact.

Canopy's connection worker still needs its sidecar to be reachable by relays in remote clusters. One QUIC endpoint serves both: it accepts the local relay's in-cluster connection and remote relays' tailnet connections without distinguishing them, because the device key is what identifies a relay either way.

### Not walking through the open door

This also means a relay could in principle reach Canopy over the public internet with no overlay at all. Not doing that: QUIC through a public load balancer is more awkward, and the tailnet supplies network-level ACL for free. Recorded because the design no longer *forbids* it, which is worth knowing if a future cluster cannot join the tailnet.

The corollary to watch in review: nothing on the listening side may assume "reached us, therefore on the tailnet". The device key is the gate, and the tailnet is now a property some connections happen to have.

## Canopy's own cluster — a relay like any other

Decided. The cluster Canopy runs in, which also hosts Tamanu test and dev instances, is read through a relay exactly as a remote cluster is: its own device and key, its own QUIC connection, its own version-naming flow. One code path, no special case, and `K8S` needs no change (it says only that Canopy can read instances in its own cluster).

The alternative — direct in-cluster reads under a widened ClusterRole on Canopy's own ServiceAccount — was rejected because it reintroduces both things this card exists to eliminate. It is a second implementation of every check, which is the drift risk that shaped the harvest design, and it hands Canopy the cluster read surface (including `secrets`, for the CNPG credentials) that the relay design exists to keep it from ever holding. Its one advantage, robustness to the relay being down, is worth little when a relay being down is already a first-class alerting condition, and the local relay is the easiest one to fix.

## Still open, deliberately

- **Relay key rotation has a window.** `Device::add_key` refuses a second active key on a device, so rotation is deactivate-then-add and the relay cannot reconnect in between. Acceptable at this fleet size; revisit if it bites.

## Spec impact to carry back

- **`DTR`** — **done**, and already on this branch: the role list names `relay` and a paragraph says what it is. The creation path is the existing provisioned-credential workflow at `role = relay`, which the implementation uses unchanged, so DTR's "how a device comes to exist" list needed nothing added.
- **`K8S`** stays accurate as written on identity: it says a relay is enrolled as a device carrying the relay role and is created, authenticated, tracked, and revoked as any other device is. The device-key decision realises that sentence rather than changing it.
- **`K8S` on the relay keeping itself current** — **decided: it rises to the spec**, and is written as "Keeping a relay current". Canopy naming the version is product behaviour, not mechanism: it decides which checks a cluster's servers get, and an operator reads the skew alert differently because of it. What went in is the behaviour plus the three properties a reimplementation must be held to — a version is named and never code sent, the cluster performs the change so a bad version leaves the previous relay serving, and the floor is the relay's own. Everything else on this card — crate layout, stream shape, ALPN, the ingestion hoist — is implementation and stays in the plan.
- **No spec change from demoting Tailscale.** `K8S` was written behavioural-only, with transport left to the plans, so it names no overlay. What it does assert still holds exactly: the relay opens its connection outward, Canopy never dials the cluster, and Canopy holds no cluster credential.
- **`SELF`** — **done**. The condition and the alert are unchanged; the wording no longer says "left behind by a fleet upgrade", because nothing upgrades a relay by hand any more. It now says what a named relay means: the version Canopy named did not take, so it wants an operator rather than another attempt at naming it.

## Decisions

### A filing is addressed in cluster coordinates, not in Canopy's identifiers

Settled while building the protocol. A filing has to say what it is about, and the obvious reading — the relay names Canopy's server UUID — would mean Canopy first pushing each relay a roster of the servers it serves, and then keeping that roster in step. That is a seventh exchange and a synchronisation problem, neither of which the card called for.

So a filing names **what the relay actually holds**: a namespace, and an instance within it (the central server, or a facility by its identity). Canopy resolves that to the server or group from the Kubernetes coordinates an operator already set on the server record, which `K8S` makes the identity anyway ("identity is set by an operator"). The relay never holds a Canopy identifier, there is no roster to push or reconcile, and an unrecognised coordinate is one filing Canopy cannot place rather than a relay out of step.

Note for review: the wire's `FilingTarget` is a *cluster coordinate*, not a second check-state scope. Canopy maps it onto the single `database::issues::Scope` on arrival — instance to the server, namespace to the group, cluster to Canopy-wide. The rule against a parallel scope enum is intact because there is still one scope vocabulary; this is the address the relay speaks, upstream of it.

### The relay announces its build on connect, and Canopy can also ask

Both, as the plan called for above, and they are not redundant. The `Hello` frame the relay opens with is what the connection registry records, so the skew alert grades from what Canopy already holds rather than a round trip per evaluation. The `Build` request re-reads it live, which is what a cluster-registration confirmation and an operator looking at a relay want.

### The push payload types stay with the HTTP endpoint

The hoist moved less than planned, for a reason worth recording. `StatusPayload` and `HealthCheck` turned out to be **documentation types only**: the handler takes `Json<serde_json::Value>` and the ingestion parses the value directly, so those structs exist to describe the endpoint in OpenAPI and nothing reads them. Moving them would have pulled `utoipa` into `commons-servers` and changed the generated spec for no gain, so they stayed where the endpoint they document lives. The push *contract* is still single-sourced: one parser, in the core, for both callers.

What moved is the ingestion itself — `parse_push` (validation, the legacy-heartbeat transform, version resolution), `ingest_push` (the ingest-mode gate and the recording transaction), and the filing beneath them. Two additions the plan had not anticipated:

- **`effective_tags_for_server` moved too**, from `public-server`'s tags module into `commons_servers::server_tags`. Grading reads the effective tags, so a relay filing has to compute them the same way a push does or the two substrates grade differently — which is the exact drift the shared-implementation design exists to prevent.
- **`kubernetes` is now a reserved source** on the push path, alongside `canopy` and `manual`. `K8S` requires it ("reserved from the device API"), the constant lives with the rest of the source vocabulary in `commons-types`, and `relay-protocol` re-exports it rather than declaring a second copy.

### One exchange for the build, not an announcement plus a query

The plan had the relay announce its build in a post-handshake control message *and* canopy be able to query it. Built as one: canopy issues `Build` as the first thing after authenticating a connection, and holds the answer in the registry entry for as long as the connection lasts.

That keeps both properties the plan wanted — the registry populated without a round trip per skew evaluation, and a live read available — while removing a message shape and, with it, an ambiguity. Stream direction alone now says what is on a stream: relay-opened unidirectional is a filing, canopy-opened bidirectional is a request. Nothing to mark and nothing to disambiguate.

### The relay's cluster work is one trait

`relay::duties::Duties` is every method canopy's requests reach: the namespace roster, the build, sleep, wake, and the version patch. Putting them behind one trait makes the list of Canopy's authority over a cluster readable in one place, and widening it a visible diff rather than something that happens when a handler reaches for a `kube` client. Note what the trait cannot express: no method returns a Kubernetes object, and there is no general read. That is the boundary the card exists to build, stated as a type.

`Unattached` implements it by answering every request with a failure while still reporting its build. So a relay that has no cluster access yet still connects, authenticates, and answers — which is what makes the transport and the enrollment testable against a real Canopy before there is a check to run, and what makes a misconfigured deployment read as "this relay cannot do anything" rather than as a silence that looks like a network fault.

The two check families fill this in. Nothing else about the relay changes when they do.

### Placing a filing waits on the identity columns

A filing names cluster coordinates and canopy resolves them against the server record's Kubernetes coordinates — but no server record carries those columns yet. They arrive with the cluster registry and the identity picker, along with the `clusters` table a coordinate would reference, so adding them here would mean building those cards inside this one.

So `jobs::relay::ingest::resolve` is the single function they fill in: it returns "cannot place" today, the listener logs each unplaceable filing with the coordinates it named, and the connection carries on. Everything downstream of resolution — both filing paths, provenance, scope mapping — is built and reachable. There is a test holding the line that an unplaceable filing costs a warning and not the connection.

Worth being plain about the consequence: **no harvest filing lands in canopy until that function is implemented.** The transport, the protocol, the identity, and the ingestion convergence are done; the last hop from a coordinate to a server row is not, and cannot be until the columns exist.

### Two latent manifest bugs fixed in passing

`commons-errors` used `diesel_async::pooled_connection` and `commons-types` used `diesel::pg::Pg` without either declaring the feature that provides it — they built only because another workspace member turned those features on, so `cargo check -p commons-types` failed on `main`. Both now declare what they use. Not part of this card's work, but building a new leaf crate off `commons-types` is what surfaced it.

## Build checklist

The check families themselves are M1 and N1; this card lays the transport, the protocol, the identity, and the ingestion path they file through. So where a check would be determined, the relay carries the seam and not the check.

### The relay role

- [x] Add `relay` to `DeviceRole` (variant, `FromStr`, `Display`), so a device can be enrolled at the role `DTR` already names.
- [x] Add the `RelayDevice` role extractor alongside the others, for the HTTP paths a relay may touch.

### `crates/relay-protocol` — the wire contract

- [x] New workspace member carrying no `kube` and no `bestool-alertd`: message types, framing, and the ALPN tokens.
- [x] ALPN version negotiation (`canopy-relay/1`): Canopy offers the range it supports, the relay picks, an incompatible pair fails at the handshake.
- [x] Length-delimited framing over a QUIC stream, with a frame ceiling so a malformed length cannot make Canopy allocate unboundedly.
- [x] The filing messages: the harvest filing (the status-push body verbatim) and the substrate filing (scope, check, observed, detail).
- [x] The three queries (namespace roster, handshake, embedded suite version), the two deployment commands (sleep, wake), and the version-naming command.

### Ingestion hoist

- [x] Move `file_health_events`, `collect_check_results`, `split_health_from_extra`, and the push payload types out of `public-server` into `commons-servers`.
- [x] Leave the axum handler a thin caller, with the existing status-push tests green across the move.

### Canopy side — the listener

- [x] QUIC listener in `crates/jobs`, its own bin, holding N inbound connections.
- [x] Client-certificate verifier that accepts any certificate, with the device-key SPKI lookup as the gate: `Device::from_key`, then the `relay` role.
- [x] Connection registry keyed by the authenticated relay device, never by anything the relay claims in a message.
- [x] Accept filings on unidirectional streams and route each family to its ingestion path.
- [x] Open the queries and commands as bidirectional streams against a registered connection.

### Relay side — the client

- [x] `crates/relay` binary: configuration (its device key, Canopy's pinned public key, the endpoint), and the reconnect loop.
- [x] Pinned verification of Canopy's certificate, on every transport.
- [x] Serve the queries and commands, and file upward, with the check determination left as the seam M1/N1 fill.
- [x] The version floor: the lowest image tag the relay will accept being told to run.

### Verification

- [x] Protocol round-trip tests: framing, the frame ceiling, ALPN mismatch.
- [x] An end-to-end test over a real QUIC connection: an enrolled relay device connects, files, and is refused when its key is unknown, deactivated, or carries another role.
