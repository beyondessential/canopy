# Relay method set — design card (K2)

Settles the relay's request/response surface, which under H1's design is the security boundary: the relay's methods take the role RBAC plays under direct cluster access.
Gates M1 and N1.
Companion to H1 (how Canopy is authorised to read a cluster) and G2 (what the harvest files); this document covers what Canopy and the relay actually say to each other.

Working notes, not yet a spec fold. Open items are listed at the end.

## The shape: an agent that files, not a proxy that answers

Both check families are check-shaped, and both are **pushed** by the relay rather than pulled by Canopy.
The relay holds the check logic for the substrate as well as the harvest, computes results in-cluster, and files them upward over its long-lived connection.
Canopy holds no method that returns a Kubernetes object.

This revises H1's candidate middle, which G2 confirmed: resource-shaped for the `kubernetes` substrate checks, check-shaped for the `alertd` harvest.
The subject demarcation G2 established is untouched — the two families still divide by what they assert something about, and they still file under two sources.
What changes is that the seam stops being a difference in method shape, so the relay is better described as Canopy's agent in the cluster than as a relay.

### Why the resource-shaped half collapsed

G2's argument for keeping the substrate side resource-shaped was that its checks are Canopy's own logic over plain objects, so Canopy could iterate on them without redeploying relays.
That advantage is worth much less than it looks, because the relay already redeploys on the fleet's cadence: it embeds `bestool-alertd`, and SELF's skew check exists precisely to keep that dependency tracking the shipped bestool.
A substrate check tweak therefore rides a release the relay was making anyway, and the iteration penalty check-shaped is supposed to carry is one this relay pays regardless.

Against that, G2's widened read set is a real cost on the resource-shaped side.
Container memory needs `metrics.k8s.io`, PVC usage needs kubelet or CSI stats, and the HTTP error-rate check must list pods in `envoy-gateway-system` and scrape a port on them.
Exposing that resource-shaped is exporting a metrics proxy to Canopy, which is the RBAC-proxy drift the relay design exists to avoid.

So the tradeoff flips on the release cadence, not on the check logic.
Worth stating outright, because if the relay ever stopped tracking bestool the argument would need revisiting.

## The surface

Three kinds of thing cross the connection, and they are worth naming separately because they carry different authority.

### Filings, pushed by the relay

Check results under both sources, produced in-cluster and filed upward.
Nothing else the relay observes crosses.

Filings converge on the ingestion path a device push takes, so parity holds by not re-deriving the filing on Canopy's side (G2).
The `alertd` harvest files what `perform_sweep` produces, verbatim.
The substrate checks file under `kubernetes` at the grain their subject warrants: a server, a server group for a namespace, or Canopy-wide with each cluster an instance.

### Queries, asked by Canopy

Only where Canopy needs an answer that is not a check.
The set is small and each entry is purpose-named rather than a generic read:

- **Namespace roster** — the central servers and facilities running in a namespace, for L1's identity picker. Joined across CNPG cluster, app workloads carrying `app.kubernetes.io/instance`, and the Gateway listener hostname, because J1 found the `facility-<N>` prefix is a positional index and neither it nor the facility id is derivable from the other.
- **Registration handshake** — is this relay connected and answering, which is what K1's "test the connection" becomes when a registered cluster is a relay identity and nothing else.
- **Embedded suite version** — the `bestool-alertd` version the relay runs, which SELF's per-cluster skew check compares against the fleet. Relay metadata, never a server figure (G2).

There is no operator-triggered re-run: the relay owns its own cadence.

### Commands, invoked by Canopy

The surface is not read-only.
Canopy can ask the relay to **hibernate** a deployment and to **wake** one.
Not immediately used, but designed in now rather than bolted on, because a mutation is exactly the kind of method that should be deliberate rather than accreted.

This is the one place the design departs from every prior document, all of which treat the relay's authority as read-only.
The relay's ServiceAccount gains the verbs hibernation takes, so its RBAC is no longer `get`/`list`/`watch` alone.
Still no `exec` and no `portforward`.

**A command targets a deployment, which is a namespace, which is a server group at a rank.**
So hibernation acts on a whole group at once rather than on one server, and there is no hibernating a single facility within a namespace.
That follows the substrate rather than being a limitation to work around: CNPG hibernation and scaling to zero are what the deploy does to itself when its TTL expires, and the deploy is the unit that has a TTL.

**Guardrails start at has-a-TTL, enforced at the relay.**
A deployment carrying no TTL cannot be hibernated, and the relay is what refuses it.
That keeps the precondition where the fact lives — a TTL is cluster-side and Canopy may not hold it — and it keeps the method set the boundary, rather than leaving the whole gate to Canopy asking correctly.
Expected to widen later, so it is a policy on the command rather than a property of the deployment.

Canopy gates production actions on top of that, as a standing principle rather than something this command introduces.
No production deployment runs on Kubernetes today, though some are planned, so the relay-side precondition is what carries the weight in the meantime.

Whether a deployment is asleep is a **fact, not a check** (G2 settled hibernation as an eligibility fact).
Canopy presents it on the group, and each affected server's checks carry it as their skip reason.
Registering a check for it would grade a deliberate act as a condition, and there is no result it could sensibly take.

## State and cadence

The relay holds current state and files on change.

Kubernetes events are an accelerant, not the trigger of record.
A check's result is level-triggered — it holds until something refreshes it, and ages into broken if nothing does — so events alone would leave Canopy holding a stale pass after a missed watch, a relay restart, or a dropped connection.
The relay therefore keeps current state from list and watch, compares each event against that state, and files when a check's result changes.
A periodic refile of everything runs regardless, so state is re-established without depending on an event arriving.

This is the informer pattern, and its payoff is latency: a pod that cannot be scheduled surfaces when it happens rather than on the next tick of a loop.

The `alertd` harvest cannot be event-driven, its readings being database queries, so it stays on a sweep cadence.
Two cadences in one relay, then: the substrate checks event-driven with a resync, the harvest on its loop.

Sleeping and waking are **admin actions and are audited**, using the same vocabulary upgrade plans and restore-replica declarations already use.

## A general log of operator actions

Wanted, and wider than these two commands.
Canopy has no such facility today: three specs say an action is audited (upgrade plans, restore-replica declarations and credential issuances, the backup recovery ceremony) and each is realised on its own terms, with no log covering operator actions across Canopy.

That is its own piece of work rather than something to fold into the relay's method set, so K2 says only that hibernation is audited and leaves the facility to a card of its own.

## Open

- Nothing outstanding on the method set itself.

## Spec impact

Folded:

- **K8S** — the relay section drops read-only, admits operator actions into the authority sentence, and gains **What crosses the connection** (the relay determines and files both families, no request returns an object, current state plus periodic refile, the three queries) and **Putting a deployment to sleep**. The substrate section now has the relay determining those checks rather than Canopy deriving them from what it reads.
- **CHK** — the `kubernetes` source is filed by a relay rather than filled by Canopy pulling, and the reserved-name wording follows.

Carried into the cards:

- **M1** — rescoped again: its checks are determined in the relay rather than in Canopy, so Canopy's side is registering the source and ingesting what the relay files. B1's breakdown entry follows.
- **J2** — the relay holds the cluster's permissions rather than read permissions, gaining the verbs sleeping a deployment takes and enforcing the expiry precondition itself. Its card also now carries what the connection is shaped to carry, since that is the protocol J2 defines.

Not rewritten:

- **H1's plan** — its RBAC section and its deferred method-set framing are superseded by this document, and it stays as the spike's record.
