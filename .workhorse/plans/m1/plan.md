# Plan: substrate checks for the cluster grain

The relay determines conditions that hold for a whole cluster and files them under the `kubernetes` source. Canopy reserves that source, lands the filings at `Scope::Cluster`, grades them, sweeps the cluster's reachability, and presents it all on a cluster page.
Three checks prove the path, each a different shape: the Tailscale API proxy (one object, present or not), node pools (several instances of one condition), and workloads broadly not running (a proportion over many objects, graded on thresholds).

Behaviour is in [K8S](../../specs/monitoring/kubernetes.md), with folds into [CHK](../../specs/monitoring/checks.md), [INC](../../specs/monitoring/incidents.md) and [FLT](../../specs/servers/overview.md).
The reasoning is in the working doc at `.workhorse/working-docs/m1/working-doc.md`. Follow-on cards are in the breakdown.

## Where the code stands

- `SUBSTRATE_SOURCE` is `"kubernetes"`, but `RESERVED_SOURCES` in `commons-types/src/namespace.rs` is still `[canopy, manual]`. So the device API accepts a push claiming `kubernetes`.
- The same gap breaks the cluster grain K1 thought it had shipped. `Namespace::of` returns `Flat` only for reserved sources, so a cluster-grain substrate filing falls through to `CheckSubject::of`, gets `Application`, finds no application type at `Scope::Cluster`, and `file_check_instances` errors. The relay end-to-end test files at `FilingTarget::Instance`, which `resolve` rejects first, so it never reaches this.
- Registering already reviewed works: `CheckPolicy::register` stamps `reviewed_at` and seeds ceiling, escalates and documentation from the filing without overwriting operator edits.
- Nothing in the relay files anything. `relay::run` is pure transport over an `mpsc::Receiver<Filing>`, and `main.rs` holds the sender open with "Nothing files yet". There is no refile timer.
- `SubstrateFiling` carries one observed/message/detail, and `ingest_substrate` hardcodes `label = String::new()`. `file_check_instances` already takes `Vec<CheckInstance>`.
- The monitor job has no cluster reachability. `kubernetes_clusters` has no `alert_when_down_for`.
- `KubernetesClusters.tsx` is registry management only. `ChecksTable`'s `CheckTarget` is `application | machine`.

## Canopy side

### Reserving the source

Add `kubernetes` to `RESERVED_SOURCES`. That one line makes the device API refuse it, makes `Namespace::of` return `Flat`, and lets cluster filings land.
The reserved-source exclusions elsewhere are hardcoded: `SourcePolicy::list_sources` has `WHERE cp.source NOT IN ('canopy', 'manual')`, and two guards in `fns/healthchecks.rs` say "the reserved canopy/manual sources have no reachability policy". Drive all three off `RESERVED_SOURCES` rather than adding a third literal. `is_reserved` is case-insensitive, so the SQL filter should compare lowercased.

Flat also settles the spun-off application grain: `pod-unschedulable` is one catalog entry fleet-wide, not one per application type.

### Cluster reachability

`Status::sweep_staleness` gains a cluster half beside the application and machine halves, built the same way: registered clusters only (drafts are skipped), `Issue::source_freshness_for_clusters`, `Issue::list_by_source_ref_for_clusters`, and `grade_reachability` filing through `raise_cluster_event_with_state`.

Migration (via `just migration`): `kubernetes_clusters.alert_when_down_for INTERVAL NOT NULL DEFAULT INTERVAL '5 minutes'` with a positive CHECK, matching the machine column's shape.
Five minutes is five missed refiles at the relay's one-minute cadence: enough to ride out the relay's pod being rescheduled or its node drained, which is routine in a cluster upgrade, and half a machine's ten because a relay's blips are shorter (it redials within 30 seconds; Canopy probes every 30).

A cluster has one expected source, `kubernetes`, so the reachability warning result can't arise for it. It presents passed, failed, or never reported.

The connection probe stays as it is (`last_answered_at`, for registration and the registry display) and does not feed reachability.

### The cluster page

- Route `/fleet/clusters/:id`, beside `/fleet/machines/:id`. The registry stays under settings and links each registered cluster here.
- A private-server fn under `fns/kubernetes_clusters.rs` (or a `fleet/clusters` module mirroring `fleet/machines`) returning the cluster, its health, its reachability, its consolidated checks, the applications it hosts, and `alert_when_down_for`. Run `just gen-openapi`.
- Build it from `MachineDetail` pieces: same header shape (title is the cluster's name alone, having no group), `HealthIndicator`, `InfoItem` "Unreachable after", `ChecksTable`, an applications list of `ServerShorty` rows, `SilencedRefsSection`, legends. See the cluster page mockup.
- `CheckTarget` gains `{ kind: "cluster"; id }`. A cluster has no group, so the table offers only the cluster-scoped silence. Check that the silence endpoints and `SilencedRefsSection` accept a cluster scope (`scoped_check_policies` already has the column).
- Admin edit for `alert_when_down_for`, following `MachineEdit`.
- The application-side link (an application's page naming its cluster in the breadcrumb, the "hasn't checked in" alert and the host nav) needs a cluster-hosted application to exist, which is N1's. Write it if it's cheap, but it can't be exercised until N1 lands.
  Not cheap as things stand: an application's page reads it through `ServerInfo`, whose `machine_id` is required (`server_to_info` expects a machine), so a cluster-hosted application cannot load there at all. Making the application grain host-agnostic is N1's, and the link goes in with it.
- Playwright coverage in `private-web/e2e/`, extending `e2e/seed.ts` to seed a registered cluster with cluster-scoped issues.

### Incidents

No change. `resolve_incident_target` already returns `None` for `Scope::Cluster`. INC now says so.

## Relay side

### Reading the cluster

The relay reads the Kubernetes API, not Prometheus: Prometheus is in these clusters but hasn't proved reliable, and a check shouldn't go broken because another monitoring system did.
Watches (e.g. `kube` runtime reflectors/watchers) give "hold current state, file on change" directly.

The relay has no deployment manifest in either repo yet. Document the read permissions these checks need alongside the relay crate (a ClusterRole listing the kinds below). Granting them is the ops repo's job.

### Watch, hold, file, refile

A cluster-check task is a producer on `client::Filings`. The transport, dispatch and reconnect loop need no change.
Per check: watch its objects, hold the current result, file when it changes, refile everything every minute (alertd's cadence, so a relay sends on one clock).
The held state is what both "file on change" and the refile read.

A permission error from the API makes the check that needed it `broken`, with the refused verb and resource in its detail. Never omit the check.

### Instances on the wire

A `file_check_instances` call is the check's complete instance set: the state reflects exactly the instances passed. So filing one instance at a time would replace the set with that instance on every filing, and the pools would clobber each other.
So `SubstrateFiling` carries every instance of its check: `instances: Vec<SubstrateInstance { label, observed, detail }>` in place of the top-level `observed` and `detail`. A single-instance check sends one instance with an empty label. `message`, `title`, the policy defaults and documentation stay per check.
`ingest_substrate` maps the vector straight onto `Vec<CheckInstance>`.
This is a breaking change to the relay protocol, and that's fine: nothing files in production yet (`main.rs` holds the sender open unused), so no relay out there speaks the old shape. It lands once, here, so the capacity card is only a check.

The relay-protocol round-trip tests cover the new shape.

### The three checks

Every check registers with `default_ceiling: Failed` (the relay's own grading decides severity; the catalog doesn't cap it) and `default_escalates: false` (inert anyway, since cluster issues open no incident). Each ships documentation seeded into the catalog.

**`tailscale-api-proxy`**: healthy when the API proxy's workload is ready and the Tailscale operator reports the proxy connected to the tailnet (its ProxyGroup / device status). Read both from the cluster. Failed otherwise, with which half failed in the detail.

**`node-pools`**: one instance per Karpenter NodePool (Karpenter 1.12). Fails when the pool's `Ready` is false or `NodeRegistrationHealthy` is false, unless `Ready` is false only because `NodeClassReady` is false: that's the node class's condition (the capacity card's check), so the pool isn't failed for it. Until that card lands, a broken node class goes unreported; that's accepted. Being near its CPU/memory limit isn't graded. Detail per instance: which condition and its message.

**`workloads-running`**: `sum(ready) / sum(desired)` across Deployments, StatefulSets, DaemonSets (`desiredNumberScheduled` / `numberReady`) and CNPG Clusters (`spec.instances` / `status.readyInstances`). Each kind gets a small reader.
- Scaled-to-zero workloads contribute zero to both sides. That covers the downscaler: environments sleep by annotating Deployments `downscaler/force-downtime=true`, which py-kube-downscaler scales to zero on its 60-second pass (see `tamanu/on-k8s/src/schedule.ts` in the ops repo). There is no namespace-level sleep marker.
- CNPG Clusters with `cnpg.io/hibernation=on` are excluded: they keep `spec.instances` while the operator removes their pods.
- Jobs and CronJobs aren't counted.
- Karpenter placeholders (two low-priority pause pods) need no exclusion.
- Detail: `healthy_share` as a number (for policy rules), plus desired and ready counts.

Grading, as a small state machine the relay holds:
- Bands: passed ≥ 90, warning 80 to 90, failed < 80.
- Hysteresis of 5 points: warning → passed needs > 95; failed → warning needs > 85; failed → passed needs > 95.
- Degrade holds, each timed from when the share went under that line and stayed there, most urgent wins: < 50 fails at once; < 70 for 2 min fails; < 80 for 5 min fails; < 90 for 5 min warns.
- Recovery isn't held.
- After a relay start (no history): file immediately only if > 95 (passed) or < 50 (failed). Otherwise don't file `workloads-running` until the governing hold has elapsed. Safe because each substrate filing stands alone (it isn't a full report), so Canopy keeps the last state and nothing recovers.

### Doc comments to correct

- `relay-protocol/src/filing.rs`: `FilingTarget` doc cites "Setting a server's identity" (gone) and calls a cluster filing "canopy-wide with the cluster as the instance" (pre-reset).
- `jobs/src/relay/ingest.rs`: the same pre-reset framing in places.
- `relay/src/lib.rs`: "both families are `alertd`'s" is true of the two application-subject families; the cluster checks live here.
- `relay/src/main.rs`: "Nothing files yet".

## Trade-offs

**Cluster checks open no incident.** A cluster belongs to no group, so a cluster-wide failure is visible on the cluster and in its health and nothing pages on it. That includes the Tailscale API proxy. If that turns out wrong, the fix is an incident target for clusters, which is its own work.

**Reachability by freshness, not by the connection.** A dropped relay is known at once through the probe, but the cluster only reads unreachable once filings go stale against the threshold. Using the connection would close that window at the cost of one target's reachability working unlike every other's, and two signals able to disagree.

**One check per component, not instances of a generic "component up" check.** Each component fails and is detected differently, so each is its own condition with its own detail and documentation. Instances are for several of one condition (node pools).

## The Tailscale API proxy in the ops setup

The ops repo (`pulumi/k8s-essentials/tailscale.ts`) runs the API proxy in-process in the Tailscale operator (`apiServerProxyConfig.mode: 'true'`), not as a ProxyGroup.
In that mode nothing in the cluster reports whether the proxy is connected to the tailnet: there is no ProxyGroup or other custom resource carrying a condition or device status, and the operator's Deployment has no readiness probe.
The operator's tailnet state lives in its `operator` Secret, which holds its node key, so reading it is not a permission to grant the relay.

Decided: support both modes.
A ProxyGroup of type `kube-apiserver` is read for both halves (`ProxyGroupAvailable`, falling back to `ProxyGroupReady`, and `status.devices` with tailnet IPs), and any one serving on the tailnet passes.
Failing that, the in-process proxy is found as a Deployment whose container sets `APISERVER_PROXY` to `true` or `noauth` (the chart's own env), graded on its readiness alone, with `connected: null` in the detail.
Neither present means no API proxy in the cluster, so nothing is filed.
The tailnet half on today's clusters waits on ops moving the proxy to a ProxyGroup.

Silencing one node pool alone is a scoped policy rule on `check.pool`, not a silence: a silence quiets the whole check. The instance path grades each instance through the rule chain, so that already works; the test case is left unticked until it has a test.

## Build steps

- [x] Reserve `kubernetes`; drive `list_sources` and the two `healthchecks.rs` guards off `RESERVED_SOURCES`
- [x] Test: device push claiming `kubernetes` is refused; Sources page doesn't list it
- [x] Test: cluster-grain substrate filing from a registered cluster lands flat at `Scope::Cluster`
- [x] `SubstrateFiling` carries `Vec<SubstrateInstance>`; `ingest_substrate` maps it onto `CheckInstance`; round-trip and ingest tests
- [x] Migration: `kubernetes_clusters.alert_when_down_for` default 5 minutes
- [x] Cluster half of `sweep_staleness`, with the two `Issue` helpers; tests for stale, recovered, never reported, draft skipped
- [x] Private API for the cluster page; `just gen-openapi`
- [x] `ChecksTable` / silence plumbing accepts a cluster target
- [x] Cluster page at `/fleet/clusters/:id`; registry links to it; admin edit of the threshold
- [x] Playwright spec for the cluster page; seed helper for a cluster and its issues
- [x] Relay: cluster-check producer with watch/hold/file/refile on a one-minute cadence
- [x] Relay: permission errors become `broken` with the refusal in detail
- [x] Relay: `tailscale-api-proxy`, both modes (see "The Tailscale API proxy in the ops setup")
- [x] Relay: `node-pools`
- [x] Relay: `workloads-running` readers, exclusions, bands, hysteresis, holds, restart handling
- [x] Relay: document the ClusterRole the checks need
- [x] Documentation for each of the three checks
  - [x] `node-pools`, `workloads-running` (`crates/relay/src/cluster/docs/`)
  - [x] `tailscale-api-proxy`
- [x] Correct the stale doc comments
- [ ] `just check`, `just test-package` for touched crates, `just typecheck`, `just test-e2e`, `cargo fmt`
