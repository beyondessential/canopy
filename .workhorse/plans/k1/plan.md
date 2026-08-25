# Cluster registry and connection — tech design

The in-app cluster registry K8S describes: a settings page where an admin registers a
Kubernetes cluster, where **registering a cluster enrols its relay** and Canopy confirms
the relay is connected and answering before the cluster is saved. Per H1 a registered
cluster **is a relay identity and nothing else** — Canopy holds no connection credential,
so there is no secret to encrypt, rotate, or persist beyond what identifies the relay.

Builds directly on J2, which laid the transport, protocol, identity, and ingestion path.
This plan is the working record for the technical approach; it decides the mechanics J2
deliberately left to this card.

## Settled coming in (from B1/H1/J2/K2)

- **A registered cluster is a relay identity.** No cluster credential is stored. "Testing a
  cluster's connection at registration time is a check that its relay is connected and
  answering" (H1).
- **A relay is a device** carrying the `relay` role, associated with no server, created via
  the existing provisioned-credential workflow (`fns/devices.rs::provision_credential`
  already mints a keypair at a chosen role and returns the private key PEM once). J2 shipped
  the `relay` role and the device-key QUIC authentication.
- **The connection registry** (`jobs::relay::registry::Registry`) is keyed by the
  authenticated relay device and is what "is that cluster connected and answering" reads
  (J2). It exposes `request()` for a live round-trip and holds each relay's `Build`.
- **`jobs::relay::ingest::resolve` is the seam this card (with L1) fills.** It maps a filing's
  cluster coordinates to a server/group/cluster placement, and needs a `clusters` table plus
  the server identity columns to exist. Until then every harvest filing is unplaceable.
- **The relayhub is a singleton QUIC-only pod** (`bin/relayhub.rs`) with no HTTP surface. The
  registry is in-memory there. Canopy's other cross-pod coordination goes through the
  database (the backups pattern), not pod-to-pod calls.

## The protocol needs nothing new

Worth stating first, because it bounds the card. `Request::Ping` / `Response::Pong` already
exist in `relay-protocol`, carrying the doc comment "Canopy confirms this before a cluster
is saved". J2 built the question this card asks; K1 adds the registry that gives it a
subject and the surface that asks it.

Note where `Ping` is answered: `relay/src/client.rs` dispatches it directly, **not through
the `Duties` trait**. So it proves the transport, the authenticated identity, and the
relay's message loop, and is deliberately independent of whether the relay has cluster
access yet. That is the right reading for registration — an `Unattached` relay, or one whose
RBAC is not yet right, still passes registration and surfaces its cluster problems as
checks. Registration confirms Canopy can *reach* the cluster's relay, not that every duty
behind it works.

## A cluster is its own table, named for what it is

**Decided: `kubernetes_clusters`.** Not `clusters` — the word is far too generic in a
codebase that also has server groups, backup repos, and CNPG clusters inside the namespaces
this very feature reads. The FK that L1 puts on `servers` is `kubernetes_cluster_id`, which
reads unambiguously next to `server_group_id`.

The row is a relay identity and a name, and nothing else:

- `id`, `name` (what an operator sees in L1's picker), `relay_device_id` → `devices`.
- No connection details, no credential, no endpoint. H1's rescope is enforced by the table
  having nowhere to put one.
- One relay per cluster, so `relay_device_id` is unique.

**Own table rather than the relay device row being the cluster.** A cluster needs identity
independent of which device currently relays it: servers reference `kubernetes_cluster_id`
and a filing resolves through it, so re-enrolling a relay (a rotation, a rebuild) must not
move every server's cluster reference. Making the device the cluster would couple the two.

## Liveness reaches the settings page through the database

**Decided.** The connection registry is in-memory in the `relayhub` pod; the settings page is
served by `private-server`. Those are separate Deployments, and Canopy's established way for
one pod to learn what another observed is the database, not a pod-to-pod call.

So **relayhub owns a probe loop**: on a cadence it `Ping`s each connection it holds and
writes what it observed against the cluster. Registration and the connectivity check both
read those rows. The private-server never talks to the relayhub.

- **Registration reads "answered within a freshness window"** rather than probing live. The
  freshness window is a fraction of a probe cadence that is already short, and at
  registration the operator has just deployed the relay, so the answer is fresh by
  construction.
- **One mechanism serves three readers**: the registration confirmation, the per-cluster
  connectivity check, and an operator looking at a cluster's row in settings.
- **Rejected: an internal HTTP surface on the relayhub** for a true live probe. It buys
  exactness that registration does not need, introduces a pod-to-pod call pattern the
  codebase does not have, and the connectivity check still needs periodic evaluation
  somewhere — so the probe loop is machinery this card needs either way, and the live path
  would be a second mechanism beside it.

Accepted cost, worth naming: "answering" becomes "answered moments ago". A relay that wedges
between probes reads as connected for up to one cadence. That is the same latency the
connectivity check has anyway, and a wedged relay is what the check is for.

## Open decisions to work

1. **The enrol-then-confirm flow / UX** — registering enrols the relay, but "connected and
   answering" can only pass after the operator has taken the minted key, deployed the relay,
   and it has dialled in. How the settings page presents that inherent two-step, and what
   happens to a minted relay device if the operator abandons the flow.
2. **Scope: does K1 build the connectivity self-alert (SELF/K8S), or is that a follow-up?**
   It reads the liveness rows this card adds and needs the `kubernetes_clusters` table.
3. **Where the K1/L1 boundary falls on `ingest::resolve`** — the cluster half is available
   once this table exists; the namespace and instance halves wait on L1's server columns.
