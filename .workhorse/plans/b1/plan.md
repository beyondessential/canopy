# Kubernetes health checks in Canopy — brainstorm

Working notes for shaping k8s monitoring in Canopy.

## September reset: read this first

The ground moved under this card while it sat. Two things landed elsewhere and between them they settle questions this plan had open and invalidate some of what it decided. Everything below this section predates them and is kept as the record of how the design got here; where the two disagree, this section wins.

**V2 landed the core model split** (canopy#V2, merged). A server is now a **machine** and an **application**; an identity is separate from both. `Scope` is `Application | Machine | Group | Global`, `servers.rs` is gone, and CHK has four targets. Two of its rules reach this card hard:

- **Applications come from reports, never from operators.** Canopy adopts what a reporter tells it, correlating by host, a key the reporter chooses, and type. **This retires L1's identity picker**: the relay reports a namespace's applications and Canopy adopts them, so there is no operator step and no roster query. J1's positional `facility-<N>` finding stops being a problem to solve and becomes the relay's business, since what a reporter derives its keys from is its own.
- **A host's checks present on its applications**, filed once at the host's scope.

V2 deliberately left one question to this project: what an application scheduled across a cluster belongs to, FLT having said an application runs on exactly one machine.

**bestool finished its side** (bestool#A2, #K2, #L2, #K1, all complete), with specs `SUBJ` (subjects) and `SUB` (substrate). It reverses a decision this card made:

- **The Kubernetes substrate implementation lives in the relay, not in `bestool-alertd` behind a feature.** This reverses G2. Their reasoning is better than ours: the shared unit is the *check* (its thresholds, outcomes and wording), and only acquisition crosses the trait boundary. bestool exposes a `Runtime` trait with systemd, podman and PM2 implementations; the Kubernetes one is ours to write in the relay.
- A check that does not apply to a subject is **absent** from its report, not skipped. Skip now means it applies but could not be read on this pass. G2 had assumed skip covered both.
- 45 checks split 18 machine / 27 application; facts split 22 / 8. A duty vocabulary replaces unit-name parsing. Per-service memory and processor are **metrics** graded against a declared ceiling, so `memory` and `load` resolved differently than G2 predicted. Postgres is an application in its own right.

**The host question, now decided.** A **cluster is a host and a check target**. An application runs on exactly one host, a machine or a cluster; a cluster carries applications of many groups, so it belongs to no group itself and an application takes its group from its namespace. Folded into FLT, CHK, SELF, and a rewritten K8S.

The consequence worth noting: **cluster reachability replaces the connectivity self-alert.** With a cluster as a target, a relay that stops answering makes its cluster and its applications unreachable under CHK's ordinary rule, so P1's escalating Canopy-wide connectivity check is no longer needed and the "while a cluster is unreadable, its checks go broken" machinery goes with it. The skew self-alert stays, being about Canopy's relay fleet rather than about a cluster.

**The branch was reconstructed, not rebased.** B1 was 705 commits behind a main that had renamed most of what its folds touched, so the branch was rebuilt on main: our own artefacts and the relay crates carried across, the folds into others' specs redone from scratch, `ingest.rs` rewritten to the new vocabulary. Full history is preserved on `b1-pre-rebase-backup`.

## Problem

Tamanu deployments on Kubernetes run no bestool (`alertd`), so they push no statuses and are invisible to Canopy's monitoring. Instead of porting bestool into k8s as an agent per server, Canopy reaches into the cluster: it runs the same alertd check suite against each server from inside the cluster, and determines its own checks about the substrate those servers run on.

## Topology (as described)

- K8s is Tamanu-only for now; not used by other products.
- A namespace = one deployment = a server group **at a particular rank** (e.g. "Nauru Demo" = Nauru group, demo rank). Multiple ranks → multiple namespaces.
- Mixed deployments are supported on principle: one rank can span some k8s servers and some on-prem servers syncing to a k8s central. Handled per-server since the k8s flag is per-server.
- No database colocation: every central and every facility has its own Postgres instance. No shared DB clusters.
- No container sharing across duties: always a central tasks container, a central sync container, central API containers (usually two), and separate facility processes/tasks. Duties never share a container.
- No dedicated substrate server; everything runs on the k8s compute pool.

## Decisions so far

- **Each k8s Tamanu server stays a Canopy server record** (target = server). No new target type. The server carries k8s coordinates: it's a k8s server, which cluster, which namespace, and for facilities a facility name/ID/prefix used to locate its DB and containers.
- **Identity is set manually, not auto-discovered** (for now). On server create/edit: "is this server in Kubernetes? → which cluster + namespace?" Canopy then queries that cluster/namespace, lists the central servers and facilities running there, and the operator picks which one this record is. Auto-discovery may come later.
- **Two sources, divided by subject** (revised by G2; this originally read "infra checks under `kubernetes`, database checks under `alertd`"). A check belongs to a source by **what it asserts something about**, not by how Canopy comes to observe it. `alertd`'s subject is one server: the thing with a database, a version, an API, sync state, and duties that ought to be running — the same subject on Kubernetes as on a VM, so the same check. `kubernetes`' subject is the substrate: what the cluster does with those workloads, at grains other than one server. The useful consequence is that **`alertd` can hold Kubernetes-only checks**: a check qualifies on its subject, not on whether it has a VM counterpart.
- **Reachability for a k8s server is Canopy-determined, not source-driven.** It's always green by default — a same-region cloud server is always reachable to Canopy, so reachability never alerts on harvest cadence (it's a coarse legacy signal; the full check suite tells you whether a server is serving). The one genuine failure is when the server's configured namespace no longer exists in the cluster: the server is gone, reported as unreachable. "Is this server up?" in the serving sense is carried by the server's own harvested checks, not reachability.
- **Harvested server checks are filed under `alertd`** (the source a VM Tamanu server uses), so a k8s server and a VM server share one catalog entry and one policy per check — a server condition means the same thing regardless of substrate. `alertd` is therefore a source with both pushed and Canopy-injected origins. Per G2 this covers the whole server-subject suite, not only the database checks.
- **Harvest failure is a single Canopy-wide failure, not per-server noise.** A Canopy-wide check per cluster covers connection + permissions (modelled like the backup-storage-identity self-alert). When Canopy can't reach a cluster, that check is the actionable failure; the affected servers' pulled/harvested checks go **broken** (unconfirmed) but are graded so they don't fail, so the fleet isn't polluted.

## Check suite

Two families per k8s server, split by subject rather than by how Canopy observes them (rebalanced by G2 — the original split put the infra list under `kubernetes` and only the database checks under `alertd`).

1. **Server checks — harvested via embedded bestool, filed under `alertd`.** The whole server-subject suite: the database conditions (sync, FHIR, migrations and the rest), plus whether duties are running and on the expected version, whether the API answers, storage headroom, and HTTP error rate. Approach: run the published `bestool-alertd` crate as a **Rust library** once per instance and file the results directly — no device-API push, no bestool binary running in-cluster. **The harvest runs inside the in-cluster relay (see Access to clusters), not a Canopy worker** — the relay connects to each instance's local `<prefix>-db-rw` (database `app`, credentials from the CNPG `<prefix>-db-app` secret, per J1) and reports only check results upward, so database credentials and query traffic never leave the cluster.
2. **Substrate checks — determined in the relay, filed under `kubernetes`.** What the cluster does with those workloads: a pod that cannot be scheduled, a volume that will not bind, and the coarser grains (a namespace, a cluster). Much smaller than originally planned, because server live, workloads ready, restarts, database up, storage and resource pressure are all server-subject and moved to the harvest.

So **M1 shrinks a long way and N1 grows to own most of what M1 was going to build**; both cards are rescoped accordingly.

**K2 then settled both families as check-shaped and pushed by the relay** (see Access to clusters), so the two no longer differ in how they reach Canopy — only in what they assert something about and which source they file under. M1's remaining Canopy-side work is registering the source and ingesting what the relay files, not deriving checks from objects it reads.

### The substrate abstraction, and why it is a bestool change

A host-subject check does not skip in a relay: the relay is a Linux process with a filesystem, memory and load of its own, so the check runs and reports **the relay pod's facts as the server's**, identically for every instance that relay serves. That passes, which is worse than failing. The fix is to give the check a *substrate* to ask instead of the local machine, keeping its graded logic and changing only the acquisition. That is a change to `bestool-alertd` itself (its Kubernetes implementation behind a feature, so a check's two behaviours cannot drift apart on separate release cycles), carried as a card in the **bestool** workspace and a prerequisite for N1.

The substrate answers more than a reading: also whether the subject is expected to be running at all (which separates a skip from a failure), the scope at which a reading is shared (per workload, per server, per cluster), and where a check's persistent state lives.

### Out of scope now

- **Backups.** K8s backups run at two layers (AWS-level, and Postgres via the CNPG Barman plugin), covered externally and not integrated in Canopy. Bringing them into Canopy is a separate future effort.
- **Cluster-infrastructure checks** (node pools, Karpenter and the like). Out of scope to build, but nothing may foreclose reporting a condition that touches a whole cluster; the Canopy-wide target with each cluster an instance is that pathway.
- **`memory` and `load` as gauges.** The Tamanu containers declare requests and no limits, so there is no per-container ceiling to take a percentage of. The condition worth alerting on is the OOM kill or eviction, an event rather than a gauge, covered by resource pressure.

## Access to clusters — resolved by H1: an in-cluster relay

The auth question is settled (H1). Rather than Canopy reaching into each cluster directly, a **small Canopy-authored relay runs in each cluster**: it holds the read permissions, connects to each local Postgres, and opens an outbound long-lived connection to Canopy. Canopy asks the relay for what it needs; the relay never accepts inbound and Canopy never talks to an external cluster's kube API directly.

- **Why the relay wins.** The capability surface becomes the relay's method set, not an RBAC surface — RBAC can't express "read this secret only to connect to that database", so any direct design hands Canopy `secrets: get/list/watch` fleet-wide. The relay also gives per-server Postgres reachability for free (dials `<prefix>-db-rw` on a ClusterIP), needs no outbound path from Canopy, and its dropped connection is a direct per-cluster connectivity signal — exactly the self-alert K8S already specifies.
- **The `alertd` harvest runs in the relay** (not a Canopy worker — revises the earlier plan): the relay embeds `bestool-alertd` as a Rust library, runs the checks against local Postgres, and reports only results up.
- **Transport:** QUIC (`quinn`). Where the tailnet is used, both ends need a **kernel-mode** Tailscale sidecar (`TS_USERSPACE=false`, `NET_ADMIN`, TUN) — userspace mode is TCP-only and QUIC won't pass. We already run QUIC over Tailscale elsewhere, so this is a known-good path, not a risk to retire. **J2 demotes Tailscale to an overlay rather than the gate:** the transport is address-agnostic, the same code dials a tailnet address or an ordinary one, and the relay in Canopy's own cluster dials Canopy's Service over cluster DNS with no sidecar at all. Nothing on the listening side may assume "reached us, therefore on the tailnet".
- **Identity:** a relay is a **device with a `relay` role**, and **J2 supersedes H1 here**: it authenticates by presenting a client certificate carrying its **device key** in the QUIC TLS handshake, which Canopy resolves by the same SPKI lookup the HTTP mTLS path uses against the same key store. This is *stronger* than the HTTP path, which terminates TLS at a proxy and so has no proof of possession; over QUIC the handshake is the proof. Consequences: SPKI pinning as a separate question dissolves (the device key *is* the pin, and a first-class record rather than a fingerprint captured at enrollment), relays differentiate by device key so a connection maps to its cluster through the device row rather than anything the relay claims, and tailnet identity stops being load-bearing for authentication.
- **Both directions authenticate.** The relay verifies Canopy too, against a pinned public key in its configuration, on every transport. J2 flips this from open: skipping it was justified by the tailnet providing peer authentication, and that justification goes with the tailnet's demotion — an endpoint a relay mistakes for Canopy can tell it which image to run.
- **Canopy stores no cluster credentials.** A registered cluster is a relay identity and nothing else — no secret at rest, no rotation to own.
- **Canopy's own cluster** (co-resident Tamanu test/dev): **decided by J2 — a relay like any other**, with its own device and key, its own connection, its own version-naming flow. One code path, no special case. Direct in-cluster reads under a widened ClusterRole were rejected because they reintroduce both things the design exists to eliminate: a second implementation of every check (the drift risk that shaped the harvest) and Canopy holding the cluster read surface, `secrets` included. Robustness to the relay being down is worth little when a relay being down is already a first-class alerting condition, and the local relay is the easiest to fix. The tailnet demotion removes what used to make this awkward — the local relay dials cluster-internally, so there is no loopback out to the tailnet and back.
- **RBAC** moves to the relay's ServiceAccount (mostly `get`/`list`/`watch`; never `pods/exec` or `pods/portforward`). **G2 widens this beyond J1's list, and past per-namespace:** container memory needs `metrics.k8s.io`, PVC usage needs kubelet or CSI stats, and the HTTP error-rate check must list pods in `envoy-gateway-system` and scrape a port on them — outside the Tamanu namespaces entirely. So the assumption that a relay reads only the namespaces of the servers it serves does not hold, and exposing that surface resource-shaped would export a metrics proxy to Canopy.
- **The relay's authority is not read-only** (K2, departing from every prior document here). It gains the verbs that put a deployment to sleep and wake it. Still no `exec`, still no `portforward`.
- **Method set: both families check-shaped and pushed** (K2, revising H1's candidate middle that G2 had confirmed). The relay holds the check logic for the substrate as well as the harvest, computes in-cluster, and files upward; no method returns a Kubernetes object, so the relay is better described as Canopy's agent in the cluster than as a relay. The resource-shaped half collapsed on **release cadence, not on check logic**: its supposed advantage was iterating on substrate checks without redeploying relays, but the relay already redeploys on the fleet's cadence because it embeds `bestool-alertd` and SELF's skew check exists to keep that tracking the shipped bestool. Worth remembering that if the relay ever stopped tracking bestool, the argument would need revisiting.
- **Beyond filings, three named queries and three commands.** Queries: the namespace roster for L1's picker, the connected-and-answering handshake for K1's registration test, and the embedded suite version for SELF's skew alert. Commands: hibernate, wake, and (added by J2's deployment decision) Canopy naming the version the relay should run. There is no operator-triggered re-run — the relay owns its own cadence.
- **Deployment and versioning: Canopy names the version, Kubernetes rolls it** (J2, resolving what H1 left open). `beyondessential/ops` deploys each cluster's relay once — namespace, ServiceAccount and RBAC, sidecar where needed, the device-key Secret, an initial image tag — and thereafter Canopy tells the relay which version it should be on, the relay patches its own Deployment's image tag, and Kubernetes pulls the signed image and rolls it. **Canopy CI never holds credentials to a cluster**, no cluster inventory lives in this public repo's CI, and standing up a cluster needs no CI change. Canopy supplies a version string, never a binary, so provenance stays with the registry and rollout safety stays with Kubernetes. This matters because K2's tradeoff (a substrate check change ships as a relay release) only holds if the deployment mechanism actually delivers that cadence.
  - Rejected: **ops deploying each roll by hand** (won't hold cadence) or **Canopy CI deploying into N clusters** (CI gains a deploy path into every cluster, and the cluster inventory becomes public). Also rejected: **a harness that receives and runs a pushed binary** — operationally attractive and it is what option C borrows from, but it is a bespoke code-distribution system to build and, decisively, it dissolves the boundary this design exists for. The relay's authority is a narrow method set where no method returns a Kubernetes object or a database row, which is why a compromised Canopy can obtain check results but not clinical data even though the relay reads every Tamanu database. A harness running pushed code widens that from "check results" to "patient data in every cluster".
  - Carried into the build: RBAC to patch **its own** Deployment and nothing else; `maxUnavailable: 0` as load-bearing rather than an inherited default, since it keeps the old pod serving and connected when a bad version fails readiness, which is what lets Canopy name an earlier version; and a **version floor** baked into the relay, answering the downgrade attack this leaves open (a compromised Canopy could otherwise order a relay back to a known-bad release).
  - Worth doing regardless: tighten Dependabot's cadence for `bestool-alertd` specifically. The default delay is calibrated for dependencies we do not control; this one we publish.
- **Cadence.** The relay holds current state from list and watch, files a check when its result changes, and refiles everything periodically so state survives a missed event, a restart, or a dropped connection. Events are an accelerant, not the trigger of record, because a check's result is level-triggered and would otherwise leave Canopy holding a stale pass. The harvest cannot be event-driven, its readings being database queries, so it stays on a sweep cadence: two cadences in one relay.
- **Cost, accepted:** a second deployable with its own release cycle in every cluster, so version skew and protocol versioning from the start. H1 left open whether that cost is warranted at a fleet of two clusters; **decided that it is**, so the relay goes ahead at the current fleet size rather than waiting for the fleet to grow to justify it. The DB harvest forces either per-facility tailnet nodes or `portforward` under any direct design, and neither is acceptable, so the relay is the design.

The **Tailscale operator's API-server proxy** (auth mode, impersonating the tailnet identity) was the leading alternative and is now a rejected one, not a fallback held in reserve: it answers only the object-read half, still needs a second mechanism for the harvest, and requires Canopy to gain tailnet egress it does not have today. Revisit only if the relay's own foundations fail (see the transport spike), not on fleet-size grounds.

## Config surface

- **Cluster registration is relay enrollment** (reshaped by H1). A Canopy settings page still owns it and it's managed in-app, but a registered cluster is a relay identity, not a set of connection details and credentials Canopy stores. "Test the connection on add" becomes "is the relay connected and answering". The per-server k8s form's cluster picker draws from this registry. (K8S spec impact — see below.)
- **DB harvest credentials never reach Canopy.** Tamanu's k8s setup uses CNPG, which stores each instance's Postgres credentials as the `<prefix>-db-app` secret in the instance's namespace (J1). The **relay** reads that secret in-cluster and connects to `<prefix>-db-rw`; Canopy receives only check results.

## Putting a deployment to sleep (new capability, from K2)

The method set is the security boundary, so a mutation belongs in it deliberately rather than bolted on later. K2 designs in two commands, hibernate and wake, not immediately used.

- **A command targets a deployment**, which is a namespace, which is a server group at a rank. So hibernation acts on a whole group at once and there is no hibernating a single facility within a namespace. That follows the substrate rather than being a limitation to work around: CNPG hibernation and scaling to zero are what a deploy does to itself when its TTL expires, and the deploy is the unit that has a TTL.
- **The guardrail starts at has-a-TTL and is enforced at the relay.** A deployment carrying no TTL cannot be hibernated, and the relay is what refuses it, keeping the precondition where the fact lives (a TTL is cluster-side and Canopy may not hold it) and keeping the method set the boundary rather than trusting Canopy to ask correctly. Expected to widen later, so it is a policy on the command rather than a property of the deployment. Canopy gates production actions on top of that as a standing principle; no production deployment runs on Kubernetes today, so the relay-side precondition carries the weight in the meantime.
- **Whether a deployment is asleep is a fact, not a check** (following G2's hibernation-as-eligibility). Canopy presents it on the group, and each affected server's checks carry it as their skip reason. Registering a check for it would grade a deliberate act as a condition, and there is no result it could sensibly take.
- **Sleeping and waking are admin actions and are audited**, in the same vocabulary upgrade plans and restore-replica declarations already use.

## Specs written on this card

This card is the tracking issue/PR; it holds the specs, and the implementation sub-cards merge into it. Specs so far:

- **New:** `monitoring/kubernetes.md` (id `K8S`) — the umbrella: deployment shape, Kubernetes servers and identity picker, the in-cluster relay and what crosses its connection, keeping a relay current, cluster registry, the subject demarcation between the two sources, harvested server checks (`alertd`), substrate checks (`kubernetes`), putting a deployment to sleep, reachability for k8s servers, and cluster-read failure handling.
- **Fold** into `private-server/self-alerts.md` — the per-cluster connectivity self-alert (escalating), now expressed as relays disconnected or not answering; plus (G2) a second per-cluster alert for check-suite version skew, reworded after J2 from "out of step with the fleet" to "not running the version Canopy named", which is what skew now means once Canopy drives the version.
- **Fold** into `monitoring/checks.md` — `kubernetes` added to the reserved sources, and Canopy-determined reachability for servers monitored through a relay. G2 restated the `kubernetes` source as substrate checks at server, namespace, or cluster grain; K2 then replaced "Canopy-populated sources" with sources **filled by a cluster's relay filing what it determines**, since nothing is pulled.
- **Fold** into `public-server/statuses.md` — `alertd` has two origins: a device push, and a cluster's relay filing for the Kubernetes servers it harvests.
- **Fold** into `private-server/device-trust.md` — `relay` added to the device roles, associated with no server.
- **No fold needed** in `private-server/server-figures.md` (`FIG`), which G2 checked: its existing rules already handle a reporter that is not on the server, provided the harvest omits rather than synthesises. A k8s server presents no bestool version because that figure is never reported for it, and a server reporting no operating system already falls back to what its database engine gives away.

### Interaction with planned upgrades (`UPG`, landed upstream)

Noticed while rebasing onto main. UPG decides a plan is met once **the group's reported version** has reached its target, so for a Kubernetes group that judgement rests entirely on the harvest reporting the Tamanu application version — there is no agent on the server to report it. G2's choice already satisfies this (the version comes from the database's recorded `currentVersion`, which the check suite falls back to when there is no install, with the container image tag as a cross-check rather than the source), so nothing needs changing. Worth recording because the coupling is not obvious from either side: if the harvest were later trimmed to omit the application version along with the host-shaped fields it must omit, Kubernetes groups would silently never meet an upgrade plan.

The spec was deliberately written behavioural-only, leaving auth mechanism and exact namespace resource names to the spikes. J1 changes nothing behavioural (all implementation detail — see `plans/j1/plan.md`).

**H1's relay is now folded into the specs**, at the architectural level (the relay exists, it dials outward, Canopy holds no cluster credential, credentials and query traffic stay in the cluster) and without the transport and deployment mechanics (QUIC/quinn, tailnet sidecars, kernel-mode networking, SPKI pinning), which stay in `plans/h1/plan.md` as implementation. What landed:

- K8S gained a **The relay in each cluster** section, reshaped the cluster registry around relay enrolment (no stored credential, connection test = relay answering), pointed the infra checks and the DB harvest at the relay, and renamed the failure section to **When Canopy cannot read a cluster** with the relay-versus-cluster diagnosis carried in the check's detail.
- SELF's Kubernetes bullet now reads as relays disconnected or not answering.
- DTR gained `relay` as a device role, associated with no server.

Both of the gaps left deliberately unspecified are now closed by J2, and neither needs a spec change:

- **Canopy's own cluster** is read through a relay like any other, so K8S staying silent on the mechanism (saying only that Canopy can read instances in its own cluster) turns out to be exactly right.
- **A relay's creation path** is the existing provisioned-credential workflow at `role = relay`: an admin has Canopy mint the keypair, the private key is handed back once and installed into the cluster as a Secret for the relay to read. DTR's "how a device comes to exist" list already covers this under **Provisioning** ("an operator has Canopy mint a credential at a chosen role"), so the list needs no relay-specific entry — the gap closes without an edit. Revocation is likewise the existing path: deactivate the key and the lookup stops resolving it.

## New findings from J1 to carry into the implementation cards

- **Positional facility prefix.** A facility's resource prefix is `facility-<N>` where N is a positional index, **not** the facility id or name. The id lives only in the `app.kubernetes.io/instance` label on app workloads and in the Gateway listener hostname. L1's picker must join across resource kinds (CNPG cluster ↔ app workloads ↔ Gateway) and **persist the prefix↔id/host binding**, because neither is derivable from the other.
- **Gateway API, not Ingress.** Query `Gateway`/`HTTPRoute`; tolerate an un-migrated namespace still on `Ingress` rather than reading the missing Gateway as "server gone".
- **Zero-replica duties are valid.** Read expected counts from each Deployment's own `.spec.replicas`; never assume a fixed count and don't alarm on a duty deliberately scaled to zero.
- ~~**TTL hibernation**~~ — **resolved by G2.** Deploys with a TTL are scaled to zero and their CNPG clusters hibernated after the window. Under the subject demarcation this is server-subject, so it belongs to the harvest, and it is not a check result at all but an **eligibility fact**: a hibernated server is deliberately asleep, so its checks report **skipped** rather than failed or broken. A hibernated namespace is still present, so the server is not gone. Specified in K8S.

## Open questions

- ~~Cluster auth mechanism~~ — **resolved by H1: in-cluster relay dialling Canopy over QUIC/tailnet.** See Access to clusters.
- ~~Exact Tamanu k8s namespace layout~~ — **resolved by J1.** See `plans/j1/plan.md`; findings carried into the cards above.
- ~~**The relay's method set**~~ — **resolved by K2: both families check-shaped and pushed**, with three named queries and two commands alongside. This overturned H1's candidate middle, which G2 had confirmed, on release-cadence grounds. See Access to clusters and `plans/k2/plan.md`.
- When to bring backups into Canopy (AWS-level + CNPG Barman) — deliberately deferred.

### Spun out: the machine / application-server split (card V2, General project)

The bestool substrate exploration surfaced a foundational modelling question that this project cannot answer on its own, so it is **card V2 outside this project**.

Canopy's `server` conflates a machine with an application server, and `device` conflates an identity with the machine it runs on. That holds only while one machine runs one workload, which is already false on some Linux hosts (bestool ignores the second) and meaningless on Kubernetes, where application servers are scheduled across a cluster and there is no machine in Canopy's sense.

**The point that makes it tractable: this is G2's subject demarcation, generalised.** G2 split checks by what they assert something about, server-subject to `alertd` and substrate-subject to `kubernetes`. On Kubernetes the substrate is the cluster and namespace; **on a VM the substrate is the machine**. So `disk_free`, `memory`, `load` and the rest are not server checks that happen to run on a host, they are substrate checks filed against a server because a server is the only target Canopy has. bestool found the same axis from its side, in registry categories that encode what inputs a check needs rather than whose host it speaks for.

**Why it blocks rather than merely relates:** if bestool gains the axis and Canopy does not, the abstraction stops at the wire — a machine-subject result from a two-workload host still has to be attributed to one of its servers arbitrarily.

Direction agreed, for V2 to explore: the current server becomes the application server; `device` retires into an identity and a machine; a machine becomes a fourth check target alongside server, group, and Canopy-wide, with its variant in the one `Scope` enum; an application server usually has one machine, sometimes shares one, and on Kubernetes has none or a cluster-class one.

Effect on this project:

- **Unaffected:** J2's relay and transport, merged and verified. The transport is agnostic to what a filing is about.
- **Contingent on V2:** L2 (bestool substrate) and N1 (harvested server checks), both of which turn on this axis.
- **Interacts:** K1 wants a Kubernetes cluster table distinct from the relay's device row, which is the same separation from the Kubernetes end; M1's filing grains may shift.

### Spun out by K2, tracked elsewhere

- **A general log of operator actions.** Wanted, and wider than hibernate/wake. Canopy has no such facility today: three specs say an action is audited (upgrade plans, restore-replica declarations and credential issuances, the backup recovery ceremony) and each is realised on its own terms, with no log covering operator actions across Canopy. K2 commits only to sleeping and waking being audited. Canopy-wide work rather than Kubernetes, so **it has its own card outside this project and is not tracked in this breakdown**. Nothing here waits on it: K8S says hibernation is audited, and the facility it eventually audits into is that card's to define.

### Left open by G2, for the implementation

- **Whether each threshold constant suits both substrates.** They were tuned against a VM. A per-check pass during N1; not a spec matter, since specs carry no thresholds.
- **Which grain the substrate reports at, per check.** The HTTP error-rate check needs a cluster-scoped reading fanned out per server where the others are per-workload, so the interface must admit both.
- **Where a check's persistent state lives.** Two alertd checks persist to one fixed path, so several instances driven in one relay process would read and write each other's state. The substrate has to carry a state location scoped to the subject.

### Settled without a spike

Two of H1's to-confirm items needed no investigation:

- **QUIC over the tailnet works.** Established practice elsewhere in our infrastructure, so the kernel-mode sidecar path is known-good statically. H1's fallbacks (tsnet at the relay end, HTTP/2 over TCP) are not on the table; note the canopy workspace itself gains `quinn` as a new dependency.
- **`bestool-alertd` is embeddable.** Published, and deliberately built as a library, so embedding it is the crate's expected use rather than an assumption to test.

What remained open was the harvest's **contract with Canopy's filing**, which G2 carried and has now landed (see `plans/g2/plan.md`). Its outcomes:

- **Parity is by construction, unless Canopy re-derives it.** The crate already builds its payload through the same serialisation a pushed bestool uses, so check names and the detail fields policy reaches as `check.<field>` match provided the relay produces the payload the same way and Canopy ingests it through the same path a device push takes. The one real risk is Canopy re-modelling the filing on its side.
- **Thresholds are compile-time constants** in each check, read from no config, so the two substrates cannot drift apart; what remains is whether a given number suits both.
- **Eligibility, not a curated subset.** Each check decides for itself whether it can run and reports skipped with a reason. Skips file normally — Canopy already carries a large proportion of skips and handles the volume in the UI rather than by withholding data.
- **The harvest reports on the server, never on the harvester.** The crate's server-wide detail is host-shaped by default and several fields are not optional, so they must be deliberately omitted rather than left to serialise the relay's hostname, OS, uptime, memory and networking as the server's. Synthesising a plausible value is worse than a gap, because it grades a real server against a fiction.
- **Version skew is relay metadata, not a server figure.** The relay's embedded suite version describes the relay, so it belongs to the cluster registry and the relay's device record, and skew becomes one comparison per cluster — a Canopy-wide check with each cluster an instance, sitting beside the connectivity check. Filing it as a server figure would put an identical row on every server a relay harvests and have an operator chase an upgrade on something that does not exist.
- **Aggregate by worst, never by sum,** where a check folds several subjects into one number. A sum hides the failure: one full volume beside an empty one of the same size reads as half full.

### Decided

- **The extra deployable is warranted** at a fleet of two clusters. The relay proceeds now; the API-server-proxy alternative is rejected rather than held as a fleet-size fallback (see Access to clusters).
- **The substrate lives in alertd behind a feature**, not as a trait the relay implements: a check's two behaviours belong in one crate so they cannot drift on separate release cycles, which is the property the reuse exists for. Cost is a `kube` dependency behind a feature and the discipline to keep it out of the default build.

**J2's tech design closed the rest** (see `plans/j2/plan.md`), so nothing on this card is now waiting on a decision:

- **Canopy's own cluster** — a relay like any other.
- **SPKI pinning** — the question dissolved: the relay authenticates with its device key, which *is* the pin.
- **How a relay is enrolled** — the existing provisioned-credential workflow at `role = relay`.
- **Who deploys the relay and how it is versioned** — ops bootstraps it once per cluster; Canopy then names the version and Kubernetes rolls it.
- **Whether the relay verifies Canopy** — yes, always, against a pinned key, on every transport.

Hibernation was settled earlier by G2 as an eligibility fact (see the J1 findings above).

### Still open after J2

- **Relay key rotation has a window.** Adding a key refuses a second active key on a device, so rotation is deactivate-then-add and the relay cannot reconnect in between. Acceptable at this fleet size; revisit if it bites.
- **Canopy's connection worker is a singleton**, so its loss makes every cluster unreadable at once. That surfaces correctly through the per-cluster connectivity check (every instance fails), but it is a single point of failure the design accepts rather than mitigates.
- **Filings are unacknowledged.** The relay gets QUIC's delivery guarantee but no application-level acknowledgement that a filing was ingested, so a filing accepted on the wire and then failed on ingest is lost until the next refile. The periodic refile is the reconciliation mechanism and already exists; revisit only if the refile interval grows long enough for the gap to matter.

## Resolved: identity stability

The namespace is the stable identity. A namespace changing means the server is gone, not that its identity drifted, so Canopy doesn't reconcile or protect against reassignment — that's an intentional operator act. The only guard needed is the reachability failure when the configured namespace is absent. A namespace disappearing and reappearing under the same name is picked back up, which is fine and intended.
