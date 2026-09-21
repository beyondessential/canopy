# Harvest contract: alertd checks filed by Canopy

Working notes for the contract between `bestool-alertd` running in the relay and what Canopy files under the `alertd` source. Gates N1 and informs the relay method set.

## What the crate already settles

Read against `bestool-alertd` 26.0.1 (`crates/alertd` in the bestool repo).

- **Parity is by construction, if Canopy does not re-derive it.** `perform_sweep(...) -> SweepResult` builds `payload["health"]` through the same `Check::to_wire()` a pushed bestool uses. A check's name is a `&'static str` in the registry and its detail fields are the same map, so a harvested filing matches a pushed one in check name and in the fields scoped policy reaches as `check.<field>`, provided the relay produces the payload via `perform_sweep` and Canopy ingests `health[]` verbatim through the same path a device push takes. The one parity risk is Canopy re-modelling the filing on its side.
- **Thresholds are compile-time constants.** `WARN_PCT_USED`, `FAIL_DEPTH`, `FAIL_OLDEST_SECS` and friends live in each check's own Rust, read from no config, no env, no file. So there is no per-server threshold surface for the harvest to supply, and no way for the two substrates' thresholds to drift apart: they are the same constants in the same build. What remains is whether one constant is the right number for both substrates, per check, which is a tuning question for the implementation rather than a contract question.
- **Selection is a library feature.** `perform_sweep` takes `selected_names` / `skip_names`, validates them against the registry, and runs the selected checks concurrently.
- **`enable_heal` is a parameter.** The harvest passes it off, which suppresses the self-heal actions (`canopy_registration`, `fhir_jobs` restarting FHIR workers) that would otherwise fire against a host the relay does not own.
- **`get_or_create_server_id()` needs handling regardless.** `perform_sweep` calls it, and it reads or writes a host file to mint a `metaServerId`. The harvest must neither create nor depend on one: a Kubernetes server's identity is the operator's selection from L1's picker.

## The substrate, not a subset

Per-check eligibility already exists and already resolves to a skip. What the crate cannot express is a second axis: the registry's categories say **what inputs a check needs** (`@tamanu` an install, `@db` any database, `host` neither) and not **whose host the check speaks for**. A relay is a Linux process with a filesystem, memory, load and IPs of its own, so a host-subject check does not skip there. It runs and reports the relay pod's facts as the server's, identically for every instance that relay serves. That failure mode is worse than a failure, because it passes.

The answer is not to turn those checks off. Most of them are asking a question that has a real Kubernetes answer, and the check's own logic is already substrate-agnostic: `memory` reads a used-bytes and a total-bytes from `sysinfo`, computes a percentage, and grades it at 90 and 98. Only the acquisition is host-specific. Give the check a **substrate** to ask instead of reaching for the local machine, and the same graded logic reports on the workloads that constitute the server.

So eligibility becomes a three-way split rather than on or off.

### Portable once a substrate can answer

- `disk_free` — the CNPG PVC's usage against `Cluster.spec.storage.size`, which J1 already identified for M1's storage check. A PVC has a real declared size, so the percentage means what it means on a VM.
- `uptime` — pod start time and container restart count.
- `version_drift` — running container image tags against the deployment's version, which in Kubernetes is a more direct reading than parsing systemd units.
- `tamanu_service` — expected duties against running workloads.
- `tamanu_http` — the relay dialling the service ClusterIP rather than the config's canonical URL.

### The concept does not exist there, so a skip is the right answer

`btrfs` (EBS, no btrfs), `time_sync` (the node's clock is EKS's concern), `external_users` (no logins, and `pods/exec` is deliberately outside the relay's RBAC), `tailscale` and `tailscale_config` (a Kubernetes server is not a tailnet node), the `caddy_*` family and `caddyfile_version` (Gateway API, no Caddy), `held_captures` (bestool's own backup holds, and Kubernetes backups are out of scope per B1), `canopy_registration` (meaningless when Canopy files directly).

These are the checks that skip today by accident of what the relay image contains. Under the substrate model they skip for a stated reason instead, which is the same outcome reached honestly.

### A different check in Kubernetes

- `memory` — **no denominator exists.** The central and facility API containers declare `requests` only and set no memory `limits` (`central/specs.ts`, `facility/specs.ts`; only the web container sets them), so those pods are Burstable and can grow to node capacity. There is no per-container ceiling to take a percentage of, and the ceiling that does exist is the node's, shared with every other pod on it including other servers' and other namespaces'. The condition worth alerting on is the OOM kill or the eviction, which is an event rather than a gauge, and it is already the `kubernetes` source's **Resource pressure** check.
- `load` — a node load average says nothing about a pod, and CPU is likewise requests-only here. Same answer as `memory`: throttling and eviction are events, not a gauge.
- `inodes` — PVC inode usage needs kubelet stats rather than the metrics API.
- `billing_tags`, `munin`, `ips` — marginal value, no clean analogue.

`http_errors` looked like it belonged here and does not: see below.

## `http_errors` against Envoy Gateway

The check reads Caddy's admin `/metrics` (Prometheus text) for `caddy_http_request_duration_seconds_count` by status code, snapshots the counters to disk each run, deltas against the oldest snapshot inside a 10 minute window, and grades the 5xx share at 5 and 20 percent. Only the acquisition is Caddy-specific, so Envoy is a direct substitute.

What the deployment actually looks like (`pulumi/k8s-essentials/envoyGateway.ts`, Envoy Gateway v1.8.3):

- **`mergeGateways: true`**, set on the `EnvoyProxy` the GatewayClass points at. So every Gateway in the cluster merges onto **one** Envoy fleet of 2 replicas in `envoy-gateway-system`, labelled `bes.gateway.role: proxy`. Not one proxy per server, and not even one per namespace.
- `ingress()` is called once per server, so each central and each facility gets its own Gateway plus five HTTPRoutes named `<server>-frontend`, `<server>-api`, `<server>-api-legacy`, `<server>-api-import`, `<server>-api-import-legacy`, all in the deployment's namespace.
- Proxy metrics are on port 19001 at `/stats/prometheus`, enabled by default, in the same Prometheus text format the check already parses.

Attribution therefore comes from the stat labels, not from which pod is scraped: `envoy_cluster_upstream_rq_xx{envoy_response_code_class="5", envoy_cluster_name="httproute/<namespace>/<route>/rule/<n>"}`. Namespace plus route-name prefix identifies the server and the suffix identifies the duty, so the harvested check can report per-server and can separate frontend from API from import, which the Caddy version on a VM cannot. The 4xx and 5xx split comes from the class label rather than from parsing status codes. `routeStatName` would add per-route virtual-host stats but is disabled by default and we do not need it.

Three complications, and they are the interesting part:

- **One scrape serves every server in the cluster.** A per-instance sweep must not scrape 19001 itself, or a tick costs (instances x replicas) scrapes of identical data. The reading is cluster-scoped and partitioned by label, then fanned out per server.
- **Counters are per pod, and the pods roll.** `counters_reset` rejects any baseline where a counter went down, which is correct for one local Caddy but wrong for a 2 replica fleet: when one pod rolls, the summed counter drops, every baseline in the window is rejected, and the check falls back to its 10 second in-run sample. The proxy pods tolerate spot (`CAN_RUN_ON_SPOT`, and prefer it), so that is frequent rather than rare. Snapshots need keying by (pod, cluster, class) with vanished pods dropped rather than read as a negative delta.
- **The snapshot history has nowhere to live.** State goes to `dirs::cache_dir()`, which in a relay pod is ephemeral and shared across every instance the relay serves.

So this check forces two axes into the substrate interface beyond "where do I read this number": **the scope at which a reading is shared** (per workload, per server, per cluster) and **where the check's own persistent state lives**. Better to design those in from the start than to discover them on the second check that needs them.

It also adds a second skew axis. The check would depend on Envoy Gateway's stat naming, which is an implementation detail of a component with its own version (pinned in ops, and the upstream issue asking for friendlier labels suggests it may move). If the expected series are absent the check cannot run, so that is **broken**, not failed, which matches the existing precedent where a class 42 SQL error reports broken rather than blaming the deployment.

## Arity turns out not to be a problem

The worry was that a VM answers with one value where a Kubernetes server answers with one per workload, so the detail fields diverge and a policy rule reading `check.percent_used` means different things on the two substrates. Checked against the actual checks, that case barely arises.

- **The multi-valued checks are already multi-valued on a VM.** A VM Tamanu also runs several services, so `version_drift` already carries `.with_detail("instances", <array>)` and `tamanu_service` already carries `.with_detail("diagnostics", <array>)`. Kubernetes workloads slot into the same arrays with no shape change at all.
- **The single-valued host-resource checks either have a real per-subject denominator or do not port.** `disk_free` has one (the PVC's declared size). `memory` and `load` do not (requests-only containers, no ceiling), so they do not port and their concern is covered as an event by **Resource pressure**.

So no instance-shaping of the VM side, and no need for CHK's "checks with instances" machinery here. Reach for it only if per-workload policy grading turns out to be wanted, which can happen later without changing the check name or its primary fields.

Where a check does aggregate several subjects into one number, **it must aggregate by worst and not by sum.** A sum hides the failure: one full PVC beside an empty one of the same size reads as half full, and the thing that is out of space is invisible. Taking the worst subject's value keeps the single-number detail shape, so parity holds exactly, and naming the worst subject in an extra detail field costs nothing, because a rule reading `check.percent_used` still means the same thing on both substrates and the VM simply has no such extra field.

## The demarcation between the two sources

K8S currently draws the line as infrastructure checks under `kubernetes` (Canopy reading the cluster) and database-derived checks under `alertd` (harvested). That line does not hold up: **workloads ready**, **restarts**, **storage** and **server live** are the same conditions as `tamanu_service`, `disk_free` and `tamanu_http`, so it would file one condition twice under two names, and FIG's fleet spread would show them as duplicate checks.

The line is not "portable versus Kubernetes-only" either. It is **what the check asserts something about**:

- **alertd's subject is one server**, as Tamanu understands a server: the thing that has a database, a version, an API, sync state, and a set of duties that ought to be running. That subject is a coherent assemblage of processes or containers regardless of what runs them, which is as true of podman on Linux as of Kubernetes, so the substrate is a real abstraction rather than a Kubernetes special case.
- **Canopy's `kubernetes` source's subject is the substrate**: the cluster's ability to run and place those workloads, at whatever grain that shows up. Coarser than a server (a namespace, a cluster, a node pool, Karpenter behaving abnormally) or finer (this pod is unschedulable, this volume will not bind).

The useful consequence is that **alertd can hold Kubernetes-only checks.** A check qualifies on its subject, not on whether it has a VM counterpart. So a check asserting something about one server that is only expressible in Kubernetes belongs in alertd anyway.

This maps onto CHK's existing three targets rather than needing a new one:

- server-subject, target server, filed under `alertd`, whether pushed or harvested;
- namespace-subject, target **server group**, since a namespace is a server group at a rank, which is what CHK already means by a group's control plane;
- cluster-subject, target **Canopy-wide**, which is where K8S already puts the cluster connectivity self-alert with each cluster an instance.

It also confirms H1's candidate middle for the relay method set, for a better reason than pragmatism: the substrate-subject checks are Canopy's own logic over plain objects, so resource-shaped, and the server-subject checks are alertd running in the relay, so check-shaped.

### What this moves

Applying the subject test to K8S's current `kubernetes` list, most of it is server-subject and moves to `alertd`: **server live** (`tamanu_http`), **workloads ready** and **restarts** (`tamanu_service`, the server's duties not staying up), **database up** (`db_connect`), **storage** (`disk_free`). **Resource pressure** stays server-subject as an event ("this server's containers are being killed") even though its cause is substrate, and it absorbs what `memory` and `load` assert on a VM.

What is left for the `kubernetes` source is genuinely substrate: the namespace being present at all, volumes failing to bind, pods that cannot be scheduled, node pool and Karpenter health, and the existing cluster connectivity self-alert.

That is a larger change to K8S than trimming, and it rebalances the work: M1 shrinks a long way and N1 grows to own most of what M1 was going to build. Both cards have been rescoped accordingly, and the bestool-side substrate work is carried as its own card in the bestool workspace, since it is a change to a different repository and a prerequisite for N1.

### It also resolves J1's hibernation question

J1 left open how a TTL-hibernated deploy is treated, having flagged that a hibernated CNPG cluster and a scaled-to-zero deployment are not a deleted namespace. Under this demarcation hibernation is server-subject, so it belongs to alertd, and it is not a check result at all: it is an **eligibility fact**. A hibernated server is deliberately asleep, so its database checks should report skipped rather than failed or broken.

So the substrate carries more than "read this number for this subject". It also answers "is this subject expected to be running right now", which is what makes the difference between a skip and a failure. `CheckContext`'s existing `has_install` and `is_tamanu` flags are ad-hoc early versions of the same idea, and the substrate should subsume them rather than sit beside them.

## Decided

- **The substrate lives in alertd, behind a feature.** The check and both its substrate implementations sit in one crate, so a new check ships with both behaviours or neither and version skew covers both at once. Cost is a `kube` dependency behind a feature and the discipline to keep it out of the default build. The alternative, a trait in alertd implemented by the relay, keeps the dependency out entirely but puts a check's two behaviours in different repos on different release cycles, where they can diverge, which is the exact failure the reuse exists to prevent.
- **The harvest files skips normally.** Canopy already carries a large proportion of skips, around a third of the checks on some servers, and deals with the volume in the UI rather than by withholding data. So the harvest reports what it ran and what it skipped like any other source, and nothing is dropped at the relay.
- **Cluster-grain checks are a pathway to keep open, not a set to build now.** The specific cluster-infrastructure checks (node pools, Karpenter, and the like) are out of scope, but nothing in these choices may foreclose reporting a condition that touches a whole cluster. The Canopy-wide target with each cluster an instance, which the connectivity self-alert already uses, is that pathway.
- **`tamanu_http` is the server-live check.** Not an overlap to resolve: under the subject demarcation it is server-subject, so it belongs to `alertd` and the `kubernetes` source carries no duplicate.

## Still open, for the implementation

- **Whether each threshold constant suits both substrates.** The numbers were tuned against a VM, so this wants a per-check pass during N1. Not a spec matter, since specs carry no thresholds.
- **Which grain the substrate reports at, per check.** `http_errors` needs a cluster-scoped reading fanned out per server, where the others are per-workload, so the interface has to admit both.

## Consequences that follow from the substrate

- **The relay's read set widens, and stops being per-namespace.** Container memory needs `metrics.k8s.io`, a different API group from the core objects J1 enumerated, and PVC usage needs kubelet stats or CSI metrics. `http_errors` needs to list pods in `envoy-gateway-system` and scrape a port on them, which is outside the Tamanu namespaces entirely: read-only and no `exec` or `portforward`, but it breaks the assumption that a relay reads only the namespaces of the servers it serves. That lands on H1's relay method-set card, and it argues for the check-shaped method surface there, since the alternative exports a metrics proxy to Canopy.
- **Per-check state is the real re-entrancy question**, not whether `perform_sweep` is self-contained. `http_errors` and `external_users` persist state to one fixed path (`dirs::cache_dir()/bestool/doctor-*.json`), so several instances driven in one process read and write each other's state. `external_users` is skipped as host-subject, so it retires on its own, but `http_errors` ports and genuinely needs its snapshot history. So the substrate has to carry a state location scoped to the subject, and the fallback when there is none is the check's existing in-run sample.

## What a push carries that a harvest supplies for itself

A pushed status carries facts bestool reads off the box. The harvest has no box, and the answer is not to synthesise a plausible `TamanuConfig`: it is for the substrate to supply each fact from the namespace, and for config-shaped questions to become observations of what is actually deployed.

- **Database URL and credentials** — the CNPG `<prefix>-db-app` secret, dialled at `<prefix>-db-rw`, database `app` (J1). Never leaves the cluster.
- **Central or facility** — `detect_kind` already prefers the database's `local_system_facts` over config, so it works with no install at all.
- **Timezone** — the `TZ` environment variable on the container, which `CONFIG_FROM_ENV` sets.
- **Application version** — `perform_sweep` already falls back to Tamanu's own recorded `currentVersion` from the database when `has_install` is false, so a harvested server reports an application version through an existing path. That is the figure FIG needs, and the container image tag is a cross-check rather than the source.
- **Facility identity** — not in the container environment at all, so it comes from the prefix-to-id binding L1's picker persists (J1's positional `facility-<N>` finding), which is Canopy's to supply.
- **`metaServerId`** — not minted and not read. `perform_sweep` calls `get_or_create_server_id()`, which reads or writes a host file; the harvest must do neither, because a Kubernetes server's identity is the operator's selection from L1's picker.
- **FHIR and worker toggles** — genuinely absent: `CONFIG_FROM_ENV` carries no FHIR keys, so there is no config to read. But whether a duty is meant to be running is observable from whether that duty's Deployment exists and what its `.spec.replicas` says, which is a better source of truth than a config file even on a VM, and is already `tamanu_service`'s territory. `fhir_config`, which compares two config toggles to each other and nothing else, has no observable counterpart and stays skipped on its existing `has_install` gate.

So the general rule: a fact the substrate can answer, it answers; a config-only question with no observable counterpart skips with a stated reason. Nothing is invented to fill a gap, because a synthesised config value grades a real server against a fiction.

## Server figures: the harvest must not describe its own host

A Kubernetes server presents **no bestool version**. The figure exists to answer whether the agent installed on a server needs upgrading, and a Kubernetes server has no bestool installed to upgrade. Filing the relay's embedded alertd version there would put an identical row in the fleet spread for every server that relay harvests, describing the harvester rather than any of those servers, and would have an operator chasing an upgrade on something that does not exist. The relay being behind would read as those servers being behind.

This is one instance of a general rule, and the rule matters more than the instance: **the harvest's server-wide detail may carry only facts about the server.** `ServerInfo` is host-shaped by default, and most of its fields describe the process that gathered them:

- `bestool_version`, `hostname`, `uptime_secs`, `cpu_cores`, `total_memory_bytes`, `os_kind`, `os_name`, `os_version`, `kernel`, `arch`, `virtualised`, `virtualisation`, `filesystems`, `ipv4`, `ipv6`, `nat64`, `os_timezone`, `node_version` — all of these would report the relay pod or its node. Several are not `Option`, so they serialise whatever the relay happens to be unless the harvest is built to leave them out.
- Legitimately about the server: `tamanu_version` (from the database's `currentVersion`), `tamanu_server_kind`, `canonical_url`, `current_sync_tick`, `timezone` (Tamanu's configured zone, not the host's), and `pg_version` (from the database).

FIG's **platform** figure reads `os_kind` and friends, so this is not only about the bestool row: a harvest that emitted them would give every Kubernetes server the relay's operating system as its own, which is worse than a wrong value because it is plausible enough to pass inspection.

The good news is that FIG's existing rules already handle a reporter that is not on the server, provided the harvest does not invent anything. A figure never reported is omitted rather than presented empty, so the bestool row is simply absent. And a server reporting no operating system falls back to the family its database engine gives away, which for a Kubernetes server resolves to non-Windows, which is both true and as fine-grained as it needs to be. So FIG likely needs no fold at all, and the work is on the harvest's side: omit, do not synthesise.

This also means the substrate reaches further into alertd than the checks. `server_info::gather` needs the same treatment, since a fact about the server has to come from the substrate exactly as a check's reading does.

## Version skew, which is not a server figure

The relay's embedded alertd version is real and worth tracking; it is just not a property of any server. It is relay metadata, so it belongs where the relay is presented: the cluster registry, the relay's device record, or the detail of the cluster-connectivity self-alert, which SELF already says carries what Canopy last observed of the relay.

Skew detection is then one comparison per cluster (the relay's embedded alertd against the bestool the fleet is running) rather than a per-server figure to eyeball. Under the demarcation above that is cluster-subject, so it is a Canopy-wide check with each cluster an instance, exactly parallel to the connectivity check it would sit beside.

The bound remains keeping the relay's `bestool-alertd` dependency tracking the fleet's shipped bestool, with a bump being a relay release on the cadence the relay's protocol versioning already needs. Under the feature-gate option that covers the substrate implementations too.
