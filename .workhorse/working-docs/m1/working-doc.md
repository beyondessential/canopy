---
status: complete
---

# Substrate checks (`kubernetes` source)

The relay determines the conditions that hold for a whole cluster and files them; Canopy reserves the `kubernetes` source, lands them at the cluster grain, grades them, and presents the cluster's health.
The application and namespace grains, and the rest of the component checks, spin off (see the breakdown).

## Scope: M1 keeps the cluster, a spun-off card takes the application grain

The card as it stood spanned two quite different problems joined only by the source name.
One is Canopy's connection to a cluster and the cluster's own health, which depends on nothing outstanding.
The other is the per-application and per-namespace grains, which cannot be built or tested until N1 creates a cluster-hosted application to file against, and whose central question — how a namespace name resolves to the group at a rank that it is — is really a question about the shape N1 settles.

So M1 keeps the connection and cluster-wide health, and the application-facing grains spin off into their own card (see the breakdown).

**M1:** the relay determining cluster-wide conditions and filing them, reserving the source, the cluster grain landing end to end, cluster reachability, and somewhere to read a cluster's health.
**Spun off:** the namespace-to-group correlation, `FilingTarget::Namespace` resolving at `Scope::Group`, and per-application substrate checks (which alertd produces).

## Where the code stands

Grounding for the design below, read off the branch rather than off K1's plan.

**The source is named but not reserved.**
`commons_types::source::SUBSTRATE_SOURCE` is `"kubernetes"` and the relay files under it, but `RESERVED_SOURCES` in `commons-types/src/namespace.rs` is still `[canopy, manual]`.
So the device API accepts a push claiming `source: "kubernetes"` today — the gate in `public-server/src/statuses.rs` reads `is_reserved`, which is false for it.

**That same gap breaks the cluster grain K1 believed it had shipped.**
`file_check_instances` derives a check's namespace through `Namespace::of(source, check, application_type)`, which returns `Flat` only when `is_reserved(source)`.
For `kubernetes` that is false, so it falls through to `CheckSubject::of(check_name)`, which answers `Application` for any name outside the machine set, and at `Scope::Cluster` there is no application type to qualify by — so it returns `None` and the filing errors out with "… is an application check, so it cannot be filed against Cluster(…)".
The relay end-to-end test never catches this because it files at `FilingTarget::Instance`, which `resolve` returns `None` for, so the filing is logged as unplaceable before it ever reaches the namespace step.
One change — adding `kubernetes` to `RESERVED_SOURCES` — closes the device-API gap and makes the cluster grain actually land, and it is the same change that makes a substrate check flat.

**Grading already reviewed is done.**
`file_check_instances` calls `CheckPolicy::register`, which stamps `reviewed_at` unconditionally and seeds ceiling, escalates, and documentation from the filing, never overwriting an operator edit.
The relay already carries `default_ceiling`, `default_escalates`, and `documentation` on `SubstrateFiling`, so "registers already reviewed with the policy its condition warrants" needs no new work here.

**The namespace grain has nowhere to resolve to.**
`resolve` returns `None` for `FilingTarget::Namespace`, and there is no namespace storage anywhere: not on `server_groups`, not on `applications`, not on `kubernetes_clusters`.
Correlating a namespace to the group it names is substantial enough, and tied enough to N1, that it spun off rather than staying here.

**Stale doc comments to fix as we go.**
`relay-protocol/src/filing.rs` still describes `FilingTarget` resolution against "the Kubernetes coordinates an operator set on the server record (spec K8S, 'Setting a server's identity')", citing a K8S section that no longer exists, and describes a cluster filing as "canopy-wide with the cluster as the instance", which the reset replaced with the cluster being its own scope.
`jobs/src/relay/ingest.rs` has the same pre-reset framing in places, and `relay/src/lib.rs` says the checks "do not live here: both families are `alertd`'s", which is true of the two application-subject families and silent on the third — the silence this card fills.
`main.rs` holds the filings sender open with the comment "Nothing files yet"; this card is what takes it.
Correct all of these alongside the change rather than leaving the protocol and relay crates describing a design that is gone.

## The cluster grain in Canopy

### A cluster's checks are read on the cluster and nowhere else

Not on the applications the cluster carries, contrary to this card's original description.
CHK and K8S both already say so, with the reasoning that settles it: a cluster schedules the applications of many groups, where a machine carries the few colocated on one box, so presenting a node-pool condition on every application would spray one condition across unrelated groups.
The machine analogy in the card description is the pre-reset framing and does not survive the cluster becoming a target in its own right.
No spec change needed — this is the specs as written, and it is the card description that was stale.

### Reserving the source

Add `kubernetes` to `RESERVED_SOURCES`, which is one line and three consequences: the device API refuses a push claiming it, `Namespace::of` returns `Flat` for it, and the cluster grain stops erroring at the namespace step.

Flat is the right namespace for a substrate check, and worth stating because it also settles the spun-off card: `pod-unschedulable` is one catalog entry fleet-wide, configured once, rather than one per application type.
A substrate condition means the same thing whichever application's pod it is.

The reserved-source exclusions are hardcoded rather than driven off the constant: `SourcePolicy::list_sources` filters with the SQL literal `WHERE cp.source NOT IN ('canopy', 'manual')`, and the two guards in `fns/healthchecks.rs` reject edits with the message "the reserved canopy/manual sources have no reachability policy".
So reserving `kubernetes` without touching those leaves it listed and editable on the Sources page.
Drive all three off `RESERVED_SOURCES` rather than adding a third literal — a source being outside source policy is exactly what reserved means, and the set should be stated once.

### Cluster reachability

**Cluster reachability is this card's.**
CHK requires every target to present a reachability check as it currently stands, whether or not a reporter has ever gone quiet, and names the relay as what reports on a cluster.
Nothing computes it today: the monitor job has no cluster handling at all, so a registered cluster presents no reachability check.

**A cluster's reachability follows filings landing, like every other target's.**
The uniform rule CHK states: reachable while a source is currently reporting, measured against the target's own threshold.
This rests on the relay refiling periodically, which nothing does today and which this card builds (see "The relay determines the cluster-wide checks").
Freshness is a real signal only because of that refile; without it a healthy cluster with nothing changing would decay to unreachable.
The connection probe stays what it is — `last_answered_at`, feeding registration and the operator display — rather than becoming a second reachability mechanism that could disagree with the first.
A consequence to accept: a cluster has exactly one expected source, `kubernetes`, so the warning result (an `on`-mode source quiet while others still report) can never arise for a cluster.
It presents passed or failed, and the check still presents and is still silenceable before anything has gone wrong, which is what CHK requires.

`Status::sweep_staleness` fans out to an application half and a machine half; a cluster half joins them, built the same way.
It needs, alongside the existing pieces: registered clusters only (a draft is a registration in progress, not a cluster), `Issue::source_freshness_for_clusters`, `Issue::list_by_source_ref_for_clusters`, and `grade_reachability` filing through `raise_cluster_event_with_state`, which K1 already shipped.

`kubernetes_clusters` has no `alert_when_down_for`, and CHK is explicit that how long a target has been quiet is measured against its own configured threshold rather than a fixed one, so that column is a migration and a control on the cluster.

A cluster that has never been filed against presents as never reported, which is the same rule every other target is held to and the honest state for a cluster whose relay has connected but filed nothing yet.

**A cluster's default down threshold is 5 minutes**, operator-settable per cluster as CHK requires.
Five missed refiles: long enough to ride out the relay's pod being rescheduled or its node drained, which is the slow case and routine during a cluster upgrade, and half the headroom a box gets because a relay's blips are shorter — it redials within 30 seconds and Canopy probes it every 30.

### The cluster detail page

**A cluster's health and checks are read on a cluster detail page.**
A page per cluster, as a machine has one, linked from the registry and from each application the cluster hosts.
That follows from checks being read on the cluster and nowhere else: without it a cluster's checks would land in the database with no surface at all, which is where they are today.
The registry page stays what it is — admin settings for registering and re-issuing — rather than growing operational monitoring into it.

The grains it presents are the cluster's own: its checks with their effective results, its health rollup, its reachability, and the per-check controls (silence, snooze, resolve, notes) that any target carries.
A cluster belongs to no group, so it carries no group-scoped silence — its checks are silenceable at the cluster and in the fleet catalog, and there is no intermediate scope.
It also lists the applications the cluster hosts, which is the navigation the fleet view cannot provide while it is organised by group.

**The applications are a list, not an enclosure of marks.**
`MachineDetail` renders "Applications (N)" as a `Stack` of `ServerShorty` rows, and an empty state that says applications appear as the machine reports them, which is exactly a cluster's case too.
The enclosure is a fleet-view construct — CHK's "a machine's mark encloses the marks of the applications on it" is about how a box is drawn among the fleet, not about its detail page.
So the question of whether an enclosure's meaning transfers to a host spanning many groups does not arise here: the detail page never used one.

**The page sits at `/fleet/clusters/:id`, beside `/fleet/machines/:id`.**
The registry stays where it is, routed under settings, because registering and re-issuing is administration while this is monitoring.
`ServerDetail` links to its host in three places — the breadcrumb's middle slot between group and application, the "hasn't checked in yet" alert, and the host nav item — and a cluster-hosted application wants a cluster in each of the same three.
Those application-side links belong with N1, which is what creates an application that has a cluster to link to; M1 builds the page and the route it links to.

Playwright coverage lands with it, per the repo's standing rule that a UI feature ships with its e2e spec.

## The relay side

### The relay determines the cluster-wide checks, and that is this card's

A cluster-wide condition — whether the node pools are healthy, and its kind — is Kubernetes-unique and has no application as its subject, so it is the relay's own to determine and file.
It is not alertd's and never will be: alertd's two families both take an application as their subject, the harvest against an instance's database and the substrate conditions about one application's workloads.
Nothing outside this repo is going to grow a check about a whole cluster.

So M1 spans the relay and Canopy, and both halves are in this repo:

- **Relay:** determine cluster-grain conditions and file them up the existing `Filings` channel, with the periodic refile that keeps a check's state current after a missed observation, a restart, or a reconnection.
- **Canopy:** reserve the source, land the filing at `Scope::Cluster`, grade it, sweep the cluster's reachability, and present it.

alertd's work stays alertd's: the per-application substrate checks belong with the spun-off card, which consumes what alertd files rather than producing anything.

alertd files every minute, and the relay's cluster checks match it — one cadence across what a relay sends, rather than two to reason about.
That also settles what a registered cluster reads before its first filing: nothing special is needed, because the relay files as soon as it is connected and running this card's checks.
A cluster that has genuinely never been heard from presents as never reported, which is the same rule every other target is held to and the honest answer for a relay that has not dialled in.

### The relay reads the Kubernetes API

Not Prometheus, which is in these clusters but has not proved useful in them.
The API also matches how K8S already describes the relay — it "holds the current state of what it watches and files a check when that check's result changes", which is watch language rather than query language, and a watch is what makes filing on change possible without polling.

### The watch-hold-file-refile loop

The seam already exists and is unused: `client::Filings` is an `mpsc::Sender<Filing>`, and `relay::run` takes the receiver, so a cluster-check task is a producer on that channel and needs no change to the transport, the dispatch, or the reconnect loop.

What it needs is a watch of the cluster objects the conditions are about, a held current state per check, a file on change, and a refile on the minute.
Holding state is what makes "file when it changes" possible and is also what the refile re-sends, so it is one structure serving both.

The relay reads the cluster with its own ServiceAccount, and K8S is explicit that a relay whose access is incomplete registers anyway and reports what it cannot do as checks.
So a permission the relay lacks is a broken result on the check that needed it, carrying what was refused, rather than a check quietly absent — absent says the condition does not exist on this cluster, which would be a false statement about a cluster nobody has finished granting.

### Checks with instances: the instance label

A cluster has several node pools, and a condition about them is one condition with an instance per pool.
CHK covers this directly under "Checks with instances": one state held for the check however many pools there are, each pool graded through policy on its own against its own detail, a rule or silence written for one pool applying to only that pool, the effective result the most urgent across pools that were not skipped, the detail naming every pool not passing, and the check recovering when none is left degraded.
That is what lets an operator silence one bad pool without quieting the check.

The protocol cannot express it.
`SubstrateFiling` carries one `observed`, one `message` and one `detail`, and `ingest_substrate` hardcodes `label = String::new()` with the comment that each substrate filing is a single unlabelled instance.
Canopy's side is already capable — `file_check_instances` takes a `Vec<CheckInstance>` and grades them individually — so the gap is the wire and the ingest, not the model.

So `SubstrateFiling` carries its instances, and `ingest_substrate` passes them through instead of the one empty-labelled instance.
It carries all of them at once, a vector of label, observed and detail, because one `file_check_instances` call is the check's complete instance set: filing a pool at a time would replace the set on every filing.
Nothing files in production yet, so reshaping the wire costs no compatibility.
Aggregating pools into one filing whose message lists them would fit the current wire, but it collapses the per-instance grading and silencing CHK requires, and the name rule forbids the other way out (`node-pool-health:ops` is a parameter spelled into a name).

This lands in M1 because node pools need it, and the spun-off card inherits it for free: several unschedulable pods on one application are instances of one check by the same reasoning.
A wire field nothing sets is a wire field nothing has shown to work, which is why node pools join the proof checks rather than waiting for the capacity card.

## The check catalogue

### What the clusters actually run

Read from the ops repo (`pulumi/k8s-core` and `pulumi/k8s-essentials`), which is where the cluster layout lives rather than in this repo or Tamanu's.

EKS. Addons: vpc-cni, kube-proxy, coredns, the EKS node monitoring agent, EBS CSI with the snapshot controller, EFS CSI, and the Mountpoint-S3 CSI driver.

Cluster controllers, each of which every namespace depends on:

- **k8s-core** — cert-manager, the AWS Load Balancer Controller, external-dns, Karpenter (controller, node classes, node pools, placeholders), opencost.
- **k8s-essentials** — Envoy Gateway with the Gateway API CRD bundle, ingress-nginx, HNC, Prometheus, CNPG with the barman-cloud plugin, the Tailscale operator, py-kube-downscaler.

Three of these change the design rather than just lengthening a list.

**Prometheus is already in the cluster.**
So there are two places a cluster condition can be read from, and the choice is structural rather than per-check: the Kubernetes API directly, or Prometheus, which already scrapes most of this and holds it over time.
This card reads the API (see "The relay reads the Kubernetes API").

**The EKS node monitoring agent is already deployed.**
Node health is therefore partly observed already, and it publishes its findings as node conditions, so a node check reads what the agent concluded rather than deriving readiness afresh.

**The Envoy Gateway and Gateway API versions are coupled, and Helm will not maintain the coupling.**
The ops repo says so at length: CRDs in a chart's `crds/` directory are installed once and never touched by an upgrade, so bumping the controller alone leaves the CRDs behind and the new controller crash-loops on a kind it gained.
`envoyGateway.ts` refuses to deploy a controller the cluster cannot run, which catches it at deploy time but says nothing about a cluster that has drifted since.
A known, written-down failure mode with a named owner is a better first check than anything invented from what Kubernetes happens to expose.

### One check per core component, each detected its own way

The unit is a core component of a running cluster, and each gets its own check because each is detected differently: an ingress controller's failure mode is not CNPG's, and neither is Karpenter's.
This is not a parameter spelled into a name, which is the thing the house rule forbids — `cnpg` and `envoy-gateway` are different conditions with different detection, different detail, and different remediation, the way `sync` and `disk_free` are different conditions rather than instances of "something is wrong".

Two consequences worth stating, because they size the card:

- Each check carries its own detection logic against whatever objects say that component is healthy, so the count of components is close to the count of the work.
- Each ships its own documentation, as all of Canopy's own checks do, and the documentation for "cert-manager is down" has nothing in common with "CNPG is down".

The instance model still earns its place elsewhere: node pools are several of one condition, detected one way, which is exactly what instances are for.

### Operator access does not run through the ingress

Cluster operator traffic goes via Tailscale, not via any ingress or gateway.
So an ingress or gateway failure is an outage for the Tamanu applications' own users and says nothing about whether anyone can get in to fix it.
The two are separate conditions with separate consequences, and conflating them would grade the wrong one as the emergency.

What carries operator access is the Tailscale operator, and its Kubernetes API proxy in particular: that is the path to the cluster's API.
Its failure is the one that leaves a cluster serving traffic while nobody can reach it to work on it, which makes it a check in its own right rather than one instance of "the Tailscale operator is up".

### The component set

**Traffic, application-facing** — Envoy Gateway, ingress-nginx, the AWS Load Balancer Controller, external-dns.
**Databases** — the CNPG operator and its barman-cloud plugin.
**Certificates** — cert-manager.
**Capacity** — the Karpenter controller; node pools, one check with an instance per pool (in M1, as a proof check); node health, read from what the EKS node monitoring agent concluded; node class validity; the spot interruption feed.
**Operator access** — the Tailscale operator, and its Kubernetes API proxy separately.
**EKS addons** — CoreDNS, kube-proxy, VPC CNI, EBS CSI with the snapshot controller, EFS CSI, Mountpoint-S3 CSI.
**Platform extras** — HNC, py-kube-downscaler, opencost, Prometheus.

That is around twenty checks, each with its own detection against whatever objects say that component is healthy, its own detail, and its own documentation.

### Two more checks, and one ruled out

**Gateway API drift** is in: the installed CRD bundle against what the Envoy Gateway controller needs.
The ops repo documents the failure at length and `envoyGateway.ts` refuses to deploy into it, so the deploy-time case is covered and the drift-after case is not watched by anything.

**Kubernetes version support** is in: the EKS version approaching end of standard support.
Slow-moving, so months of warning is the whole value, and the alternative to a check is noticing when AWS starts charging extended-support rates.

**IP headroom is out.**
The clusters are IPv6-only with NAT46, so the IPv4 exhaustion this would warn about is not a wall these clusters are anywhere near — it would take an implausible number of nodes to approach it.
No IPv6 equivalent replaces it, address space not being the constraint there.

### Checks derived from failures actually had

A set derived from real incidents beats one derived from what Kubernetes exposes, so these take precedence over anything in the component set above that was reasoned from the component list.

**Node pressure and moribund nodes** confirm the node health check rather than changing it, and sharpen what it reads: memory and ephemeral disk pressure specifically, and a node alive but not functioning.
A moribund node warns straight away and fails if it is still moribund after Karpenter should have replaced it, which separates a node being handled from Karpenter not handling it.

**EC2 node classes out of date and unschedulable** is a check the set did not have.
It is a different condition from a node pool being unhealthy — the pool is willing and the class it references cannot launch — and the ops repo keeps them as separate objects (`karpenter/classes.ts` beside `karpenter/pools.ts`), so they are separate checks against separate kinds.

**Karpenter unable to read spot feeds** is also new.
Without the interruption feed Karpenter cannot drain a node before AWS reclaims it, so the consequence is abrupt pod loss rather than degraded provisioning, and nothing else reports it.

**CNPG running out of disk space** is the one that does not belong at this grain.
A CNPG cluster is one application's Postgres — a namespace holds one instance per central and per facility — so its disk headroom is an application-subject condition, not a cluster-wide one.
It is recorded on this card as explicitly not this card's, and it fits the application grains rather than anything here.

### Node pools: what the check grades on

Karpenter 1.12 reports Ready, NodeClassReady and NodeRegistrationHealthy conditions on each NodePool.
A pool fails when it is not Ready or the nodes it launches are not registering.
A pool that is not Ready only because its node class is broken is left to the capacity card's node class check, so one cause raises one alert rather than two.
Until that card lands, a broken node class goes unreported, which is accepted.
Being near a pool's CPU or memory limit isn't graded here.

### The Tailscale API proxy: what healthy means

Healthy when the proxy's workload is ready and the Tailscale operator reports the proxy as connected to the tailnet.
Readiness alone would miss a proxy that is up in the cluster but unreachable over the tailnet, and nobody being able to get in is the failure this check exists for.
Both are read from inside the cluster.

### The aggregate: broadly unscheduled or failing

"Sometimes a problem is detected because we notice we've got like 25% unscheduled or failing" is the most valuable thing in the list, and the set had nothing like it.

It is genuinely cluster-grain rather than a rollup of per-application checks.
A per-application check says this application's pod cannot be placed; this says a quarter of everything cannot, which is a different signal with different causes — the cluster out of capacity, Karpenter wedged, a node class broken — and it fires for causes no component check anticipated.
That is the point of it: it is the check that catches what the others did not think of, which is exactly the failure mode described, where the condition was noticed by eye rather than reported.

#### What it counts

Ready replicas over desired replicas, summed across Deployments, StatefulSets, DaemonSets and CNPG clusters.
Counting against what each workload wants, not the pods that exist, catches pods that were never created: a ReplicaSet blocked by quota, or by an admission webhook that's down.
A broken webhook does exactly that, and pod counting can't see it because no pod exists to count.
Each kind reports desired and ready in its own fields, so each gets its own small reader.

What drops out, so the check doesn't alarm every night:

- **Scaled to zero**, whether an operator did it or the downscaler did it to sleep an environment. Desired is zero, so it adds nothing to either side and needs no special case.
- **Hibernated CNPG clusters.** A cluster with `cnpg.io/hibernation=on` still asks for its instances while the operator has removed their pods, so it has to be recognised by that annotation and left out. Without this, every sleeping environment's databases would count as failing.
- **Jobs and CronJobs.** They aren't meant to keep running, so they're not in the count at all.

#### How an environment actually sleeps

There is no namespace-level fact to read.
In the ops repo (`tamanu/on-k8s/src/schedule.ts`), hibernating an environment annotates each Deployment with `downscaler/force-downtime=true`, which py-kube-downscaler then scales to zero on its next pass (every 60 seconds), and sets `cnpg.io/hibernation=on` on each CNPG cluster, which the CNPG operator hibernates by removing its pods.
The namespace is left unannotated on purpose, because a namespace-level downscaler annotation would override the workload-level one.
So there is no "sleeping namespace" to exclude. Each object that sleeps is recognised by its own mark, which is what "What it counts" does.

Karpenter's placeholders (one low-priority pause pod each for the system and ingress purposes) go pending by design when their node is disrupted, until Karpenter provisions a replacement.
They need no exclusion: two pods is about 2% of the smallest cluster and they're pending only briefly, and a placeholder stuck pending is a real signal that Karpenter isn't provisioning.

Graded on the **healthy share** — ready replicas as a proportion of desired — rather than on the failing share, which is how it reads on the check and how the thresholds below are stated.

The smallest cluster runs 92 pods, so one pod is a bit over 1% and the proportion does not jump around at the small end.
That settles the shape: a proportion alone, with no absolute-count fallback and no floor below which the check stays quiet.
Neither would earn its place when the smallest real denominator is already this large, and both would be machinery sized for a cluster that does not exist.

#### Bands and hysteresis

Three bands, with edges at 80% and 90% healthy:

- **passed** at 90% or above
- **warning** from 80% up to 90%
- **failed** below 80%

Each edge carries 5 points of hysteresis, so getting back across an edge takes 5 points more than falling across it did.
A warning passes again only once the share is back above 95%, and a failure only lifts to a warning above 85%.
Between those, the check stays at the result it last held, so a share sitting right on an edge can't flap.

On the smallest cluster (92 pods), that is about ten pods not running to warn and nineteen to fail.

#### How long a degradation must last, by how deep it is

The deeper the drop, the sooner it counts:

- **below 50%**: fails at once
- **below 70%** for 2 minutes: fails
- **below 80%** for 5 minutes: fails
- **below 90%** for 5 minutes: warns

Each is a condition of its own, measured from when the share first went under that line and stayed there, and the check takes the most urgent that holds.
So a share that crashes to 40% fails at once, one that sits at 65% fails after 2 minutes, and one that drifts to 85% warns after 5.
Every tier below 80% ends in failure: the 5-minute tier is the only route to a warning.

A shallow dip is the rolling-deploy case and needs time to prove itself.
A deep one isn't something a deploy does, so waiting would only delay the alarm.

Recovery isn't held.
The hysteresis already makes recovery a deliberate move, and holding it as well would keep a cluster showing failed for minutes after it had plainly come back.

#### After a relay restart

The relay keeps the hold timers and the hysteresis state in memory, so a restart loses them.
A relay restarting with no history would read a share of 85% as passing, because no hold has built up yet.
It would file that, recover the open issue, and then warn again five minutes later: a flap caused only by the relay restarting, which happens routinely during cluster upgrades.

So after a restart the relay files at once only where the result doesn't depend on history: above 95% is passed whatever came before, and below 50% is failed.
Anywhere between, it files nothing for the aggregate until the hold that would decide it has passed.
Holding back is safe because each substrate filing is its own check rather than a full report, so an unfiled check is not taken to have recovered and Canopy keeps its last state in the meantime.
The cluster stays reachable throughout, because the relay's other checks keep filing.

#### Regrading by operators

These thresholds are the relay's defaults, not per-cluster configuration.
K8S already puts a check's thresholds in its implementation and has operators grade through policy.
The aggregate puts the healthy share in its detail as a number, so a catalog rule on `check.healthy_share` can regrade it fleet-wide; policy rules already compare numbers.
The model can hold a rule at cluster scope too, but the interface only offers silences at scope, so per-cluster regrading is not something this card offers.

## Trade-offs

**Cluster-wide checks open no incident, and that is the shipped behaviour rather than a gap this card closes.**
A cluster belongs to no group, so `resolve_incident_target` returns `None` at `Scope::Cluster` and the state is recorded and rolls into the cluster's health without opening an incident.
Worth naming plainly because it is the consequence of the grain rather than an oversight: a cluster-wide failure is visible on the cluster and in its health, and nothing pages on it.
If that turns out to be wrong, the fix is an incident target for a cluster, which is its own piece of work.

**Reachability by freshness rather than by the connection accepts a window.**
A relay that drops its connection is known to be gone at once through the probe, but the cluster does not read unreachable until its filings go stale against the threshold.
Taking the connection as the input would close that window, at the cost of computing one target's reachability by a mechanism no other target uses, and of two signals that can disagree about the same cluster.
One rule, one clock, one answer is worth more than the seconds.

## Open questions

None outstanding for M1.

Moribund nodes were settled for the capacity card: they warn straight away and fail if still moribund after Karpenter should have replaced them.
How long that is belongs to the capacity card, which carries the note.

## Testing notes

### Canopy side

- A substrate filing at cluster grain from a registered cluster's relay lands as an issue at `Scope::Cluster`, with the check flat rather than application-namespaced. This is the case the current end-to-end test does not reach, and it fails today.
- A device push claiming `source: "kubernetes"` over the device API is rejected.
- The Sources page does not list `kubernetes`, and an attempt to set a reachability or ingest mode on it is refused.
- A substrate check registers already reviewed, at the ceiling and escalates the filing named, and a later filing does not overwrite an operator's edited policy.
- A registered cluster whose relay stops filing reads unreachable once past its threshold, and recovers when filings resume.
- A registered cluster that has never been filed against presents as never reported, not as unreachable.
- A cluster that is only a draft is swept for nothing and presents no reachability.
- The cluster detail page presents the cluster's checks, health and reachability, and silencing a check there quiets it on the cluster.
- A cluster-grain issue opens no incident.
- Observe the aggregate on the smallest cluster before relying on it, including across a scheduled sleep, a wake (many pods pending at once while Karpenter provisions nodes), and a cluster upgrade, and adjust the defaults if the bands turn out to be wrong in practice.

### Relay side

- A cluster condition changing files once, promptly, rather than on the next refile.
- An unchanged condition refiles on the cadence, so a cluster stays reachable while nothing is happening.
- A filing attempted while the connection is down is not lost to the connection: the next refile carries the current state.
- A permission the relay's ServiceAccount lacks produces a broken result naming what was refused, not an absent check.
- The aggregate's bands: 89% held 5 minutes warns; 79% held 5 minutes fails; 65% held 2 minutes fails; 45% fails at once.
- The aggregate's hysteresis: a warning at 92% stays a warning, and passes at 96%; a failure at 83% stays failed, and lifts to warning at 86%.
- A dip below 90% that recovers inside 5 minutes files nothing.
- After a relay restart at 85% healthy, the aggregate files nothing until the hold has passed, and the existing issue stays open rather than recovering and reopening.
- After a relay restart at 97% or at 40%, the aggregate files at once.
- The aggregate leaves out workloads scaled to zero (by an operator or by the downscaler), CNPG clusters with `cnpg.io/hibernation=on`, and Jobs.
- A Deployment whose pods can't be created (quota exceeded, admission webhook down) counts against the aggregate even though none of its pods exist.
- A sleeping environment, with its Deployments scaled down and its CNPG clusters hibernated, contributes nothing to either side of the aggregate.
- A check with instances (node pools) files each instance under its own label, grades each on its own policy, takes the effective result as the most urgent across them, and lists every instance not passing in the detail.

## How this card is split

Plumbing first, then a card per area.

**M1** takes the plumbing end to end plus checks to prove it: reserving the source and driving the reserved-source exclusions off the constant, the instance label on `SubstrateFiling` and through `ingest_substrate`, the relay's watch-hold-file-refile loop, cluster-grain ingest and grading, the cluster reachability sweep with its threshold column, and the cluster detail page.

Three checks prove it, chosen because their shapes differ and each exercises something the others do not.

**The Tailscale Kubernetes API proxy** is a single component, present and serving or not.
It proves the plain path: watch one thing, hold its state, file on change, refile on the minute.
It is also the check most worth having on day one, being the failure that leaves a cluster serving while nobody can reach it to work on it.

**The aggregate** is a proportion computed across every workload in the cluster, with exclusions and a duration hold before it grades.
It proves what the other cannot: watching many objects of several kinds, deriving one result from them, and grading on thresholds rather than on presence.
It is also the check that catches what the specific checks did not anticipate, which is the failure mode that has actually been caught by eye.

**Node pools** are several of one condition, detected one way, filed as one check with an instance per pool.
It proves the instance path: the label on the wire, per-instance grading and silencing, detail naming every pool not passing, and the effective result taking the most urgent of them.

Between them the plumbing is exercised by a check that is nearly nothing, a check that is nearly everything, and a check that is several of the same thing at once — the three shapes a substrate filing comes in. Three of one shape would have tested it far less.

Then one card per area, each independently reviewable and mergeable, and able to run in parallel once the pattern is set:

- **Traffic** — Envoy Gateway, ingress-nginx, the AWS Load Balancer Controller, external-dns, and Gateway API drift.
- **Databases and certificates** — the CNPG operator, its barman-cloud plugin, and cert-manager.
- **Capacity** — the Karpenter controller, node health from the EKS node monitoring agent, EC2 node class validity, and Karpenter's spot interruption feed. Node pools are not here, having moved into M1 as the proof check for the instance path.
- **Addons** — CoreDNS, kube-proxy, VPC CNI, EBS CSI with the snapshot controller, EFS CSI, Mountpoint-S3 CSI, and Kubernetes version support.
- **Platform extras** — HNC, py-kube-downscaler, opencost, Prometheus, and the Tailscale operator beyond its API proxy.
