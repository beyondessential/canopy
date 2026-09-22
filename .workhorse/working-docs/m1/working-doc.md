---
status: draft
---

# Substrate checks (`kubernetes` source)

Worker-side companion to the relay's substrate checks: register the `kubernetes` source as reserved, ingest what the relay files under it at all three grains (application, namespace, cluster), and grade it.

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
Correlating a namespace to the group it names is the substantive unshipped piece of this card.

**Stale doc comments to fix as we go.**
`relay-protocol/src/filing.rs` still describes resolution against "the Kubernetes coordinates an operator set on the server record (spec K8S, 'Setting a server's identity')" and cluster filings as "canopy-wide with the cluster as the instance".
Both describe the pre-reset design; the spec section they cite no longer exists.

## Scope: M1 keeps the cluster, a spun-off card takes the application grain

The card as it stood spanned two quite different problems joined only by the source name.
One is Canopy's connection to a cluster and the cluster's own health, which depends on nothing outstanding.
The other is the per-application and per-namespace grains, which cannot be built or tested until N1 creates a cluster-hosted application to file against, and whose central question — how a namespace name resolves to the group at a rank that it is — is really a question about the shape N1 settles.

So M1 keeps the connection and cluster-wide health, and the application-facing grains spin off into their own card (see the breakdown).

**M1:** reserving the source, the cluster grain landing end to end, cluster reachability, and somewhere to read a cluster's health.
**Spun off:** the namespace-to-group correlation, `FilingTarget::Namespace` resolving at `Scope::Group`, and per-application substrate checks.

## Decisions

**A cluster's checks are read on the cluster and nowhere else.**
Not on the applications the cluster carries, contrary to this card's original description.
CHK and K8S both already say so, with the reasoning that settles it: a cluster schedules the applications of many groups, where a machine carries the few colocated on one box, so presenting a node-pool condition on every application would spray one condition across unrelated groups.
The machine analogy in the card description is the pre-reset framing and does not survive the cluster becoming a target in its own right.
No spec change needed — this is the specs as written, and it is the card description that was stale.

**Cluster reachability is this card's.**
CHK requires every target to present a reachability check as it currently stands, whether or not a reporter has ever gone quiet, and names the relay as what reports on a cluster.
Nothing computes it today: the monitor job has no cluster handling at all, so a registered cluster presents no reachability check.

**A cluster's reachability follows filings landing, like every other target's.**
The uniform rule CHK states: reachable while a source is currently reporting, measured against the target's own threshold.
The relay refiles what it holds periodically, so freshness is a real signal rather than one that decays whenever a cluster is healthy.
The connection probe stays what it is — `last_answered_at`, feeding registration and the operator display — rather than becoming a second reachability mechanism that could disagree with the first.
A consequence to accept: a cluster has exactly one expected source, `kubernetes`, so the warning result (an `on`-mode source quiet while others still report) can never arise for a cluster.
It presents passed or failed, and the check still presents and is still silenceable before anything has gone wrong, which is what CHK requires.

**A cluster's health and checks are read on a cluster detail page.**
A page per cluster, as a machine has one, linked from the registry and from each application the cluster hosts.
That follows from checks being read on the cluster and nowhere else: without it a cluster's checks would land in the database with no surface at all, which is where they are today.
The registry page stays what it is — admin settings for registering and re-issuing — rather than growing operational monitoring into it.

## Implementation notes

### Reserving the source

Add `kubernetes` to `RESERVED_SOURCES`, which is one line and three consequences: the device API refuses a push claiming it, `Namespace::of` returns `Flat` for it, and the cluster grain stops erroring at the namespace step.

Flat is the right namespace for a substrate check, and worth stating because it also settles the spun-off card: `pod-unschedulable` is one catalog entry fleet-wide, configured once, rather than one per application type.
A substrate condition means the same thing whichever application's pod it is.

The reserved-source exclusions are hardcoded rather than driven off the constant: `SourcePolicy::list_sources` filters with the SQL literal `WHERE cp.source NOT IN ('canopy', 'manual')`, and the two guards in `fns/healthchecks.rs` reject edits with the message "the reserved canopy/manual sources have no reachability policy".
So reserving `kubernetes` without touching those leaves it listed and editable on the Sources page.
Drive all three off `RESERVED_SOURCES` rather than adding a third literal — a source being outside source policy is exactly what reserved means, and the set should be stated once.

### Cluster reachability

`Status::sweep_staleness` fans out to an application half and a machine half; a cluster half joins them, built the same way.
It needs, alongside the existing pieces: registered clusters only (a draft is a registration in progress, not a cluster), `Issue::source_freshness_for_clusters`, `Issue::list_by_source_ref_for_clusters`, and `grade_reachability` filing through `raise_cluster_event_with_state`, which K1 already shipped.

`kubernetes_clusters` has no `alert_when_down_for`, and CHK is explicit that how long a target has been quiet is measured against its own configured threshold rather than a fixed one, so that column is a migration and a control on the cluster.

A cluster that has never been filed against presents as never reported, which is the same rule every other target is held to and the honest state for a cluster whose relay has connected but filed nothing yet.

### The cluster detail page

The grains it presents are the cluster's own: its checks with their effective results, its health rollup, its reachability, and the per-check controls (silence, snooze, resolve, notes) that any target carries.
A cluster belongs to no group, so it carries no group-scoped silence — its checks are silenceable at the cluster and in the fleet catalog, and there is no intermediate scope.
It also lists the applications the cluster hosts, which is the navigation the fleet view cannot provide while it is organised by group.

Playwright coverage lands with it, per the repo's standing rule that a UI feature ships with its e2e spec.

### Doc comments to correct

`relay-protocol/src/filing.rs` still describes `FilingTarget` resolution against "the Kubernetes coordinates an operator set on the server record" citing a K8S section that no longer exists, and describes a cluster filing as "canopy-wide with the cluster as the instance", which the reset replaced with the cluster being its own scope.
`jobs/src/relay/ingest.rs` has the same pre-reset framing in places.
Correct them alongside the change rather than leaving the protocol crate describing a design that is gone.

## Trade-offs

**Cluster-wide checks open no incident, and that is the shipped behaviour rather than a gap this card closes.**
A cluster belongs to no group, so `resolve_incident_target` returns `None` at `Scope::Cluster` and the state is recorded and rolls into the cluster's health without opening an incident.
Worth naming plainly because it is the consequence of the grain rather than an oversight: a cluster-wide failure is visible on the cluster and in its health, and nothing pages on it.
If that turns out to be wrong, the fix is an incident target for a cluster, which is its own piece of work.

**Reachability by freshness rather than by the connection accepts a window.**
A relay that drops its connection is known to be gone at once through the probe, but the cluster does not read unreachable until its filings go stale against the threshold.
Taking the connection as the input would close that window, at the cost of computing one target's reachability by a mechanism no other target uses, and of two signals that can disagree about the same cluster.
One rule, one clock, one answer is worth more than the seconds.

## Testing notes

- A substrate filing at cluster grain from a registered cluster's relay lands as an issue at `Scope::Cluster`, with the check flat rather than application-namespaced. This is the case the current end-to-end test does not reach, and it fails today.
- A device push claiming `source: "kubernetes"` over the device API is rejected.
- The Sources page does not list `kubernetes`, and an attempt to set a reachability or ingest mode on it is refused.
- A substrate check registers already reviewed, at the ceiling and escalates the filing named, and a later filing does not overwrite an operator's edited policy.
- A registered cluster whose relay stops filing reads unreachable once past its threshold, and recovers when filings resume.
- A registered cluster that has never been filed against presents as never reported, not as unreachable.
- A cluster that is only a draft is swept for nothing and presents no reachability.
- The cluster detail page presents the cluster's checks, health and reachability, and silencing a check there quiets it on the cluster.
- A cluster-grain issue opens no incident.

## Open questions

- [ ] What is the default `alert_when_down_for` for a cluster? A relay's connection is continuous and it refiles periodically, so the refile cadence is the floor; a machine's default is probably not the right borrow.
- [ ] Does the cluster detail page present the cluster's applications as marks (an enclosure, the way a machine's page does) or as a plain list? The enclosure carries "one subject per mark" meaning that may not transfer to a host spanning many groups.
- [ ] Where is the cluster detail page reached from, beyond the registry? An application hosted on a cluster needs a link to its host, as one on a box links to its machine.
