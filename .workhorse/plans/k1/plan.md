# Cluster registry and connection — tech design

The in-app cluster registry K8S describes: a settings page where an admin registers a
Kubernetes cluster, where **registering a cluster enrols its relay** and Canopy confirms the
relay is connected and answering before the cluster is saved. A registered cluster is a
relay identity and a name; Canopy holds no connection credential, so there is nothing to
encrypt, rotate, or persist beyond what identifies the relay.

Builds on J2, which laid the transport, protocol, identity, and ingestion path. This plan is
the working record for the technical approach; it decides the mechanics J2 deliberately left
to this card.

## Settled coming in (from B1/H1/J2/K2)

- **A registered cluster is a relay identity.** No cluster credential is stored. Testing a
  cluster's connection at registration is a check that its relay is connected and answering.
- **A relay is an identity** carrying the `relay` role, attached to no machine, created via
  the existing provisioned-credential workflow (`fns/devices.rs::provision_credential` mints
  a keypair at a chosen role and returns the private key once). J2 shipped the role and the
  device-key QUIC authentication.
- **The connection registry** (`jobs::relay::registry::Registry`) is keyed by the
  authenticated relay identity and exposes `request()` for a live round trip.
- **`jobs::relay::ingest::resolve` is the seam this card fills.** Its doc comment names this
  card's work as the blocker: an application runs on exactly one machine today, and a
  relay's applications have none.
- **The relayhub is a singleton QUIC-only pod** (`bin/relayhub.rs`) with no HTTP surface. The
  registry is in-memory there. Canopy's cross-pod coordination goes through the database.

## The protocol needs nothing new

`Request::Ping` / `Response::Pong` already exist in `relay-protocol`, carrying the doc
comment "Canopy confirms this before a cluster is saved". J2 built the question this card
asks; K1 adds the registry that gives it a subject and the surface that asks it.

Note where `Ping` is answered: `relay/src/client.rs` dispatches it directly, **not through
the `Duties` trait**. So it proves the transport, the authenticated identity, and the relay's
message loop, and is deliberately independent of whether the relay has cluster access yet.
That is the right reading for registration — an `Unattached` relay, or one whose RBAC is not
yet right, still passes registration and surfaces its cluster problems as checks.
Registration confirms Canopy can reach the cluster's relay, not that every duty behind it
works.

## A cluster is its own table, named for what it is

**Decided: `kubernetes_clusters`.** Not `clusters` — far too generic in a codebase that also
has server groups, backup repos, and the CNPG clusters inside the namespaces this feature
reads. The FK is `kubernetes_cluster_id`, which reads unambiguously next to `server_group_id`.

The row is a relay identity and a name, and nothing else:

- `id`, `name` (what an operator sees in the picker), `relay_identity_id` → `devices`.
- No connection details, no credential, no endpoint. The rescope is enforced by the table
  having nowhere to put one.
- One relay per cluster, so `relay_identity_id` is unique.

**Own table rather than the relay identity row being the cluster.** A cluster needs identity
independent of which relay currently serves it: applications reference
`kubernetes_cluster_id` and a filing resolves through it, so re-enrolling a relay must not
move every application's cluster reference.

Naming note to confirm in review: the card calls the column `relay_identity_id`, matching
FLT's "identity" vocabulary, while the table it references is still `devices`. Keeping the
card's name, since FLT is where the vocabulary is settled.

## A cluster is a host, and that is the schema's hard blocker

`applications.machine_id` is `Uuid` and NOT NULL, so an application *requires* a machine.
FLT now says an application runs on exactly one **host**, a machine or a cluster, and that an
application on a cluster has no box of its own. So:

- `machine_id` becomes nullable, a `kubernetes_cluster_id` joins it, and a CHECK holds
  exactly one of the two.
- A cluster belongs to no group, carrying applications of many groups at once; an
  application on a cluster takes its group from its namespace rather than from an operator
  (FLT, "Groups").

This is what unblocks every other card in the project, and it is the larger half of this
card's model work — bigger than the registry table itself.

## A cluster is a check target

`Scope` is `Application | Machine | Group | Global`, stored as one nullable FK column per
targetable grain (`application_id`, `machine_id`, `server_group_id`, all null being
canopy-wide) under a `num_nonnulls` CHECK holding it to at most one.

AGENTS.md states the recipe exactly, so this is mechanical rather than a design choice:
adding a grain means **a variant, a column, and an arm in each of `to_columns`,
`from_columns`, and `resolve_incident_target`** — never a second enum and never a
hand-written match over the storage columns. So `Scope::Cluster` adds a
`kubernetes_cluster_id` column to the scoped tables, widens the CHECK, and takes an arm in
all three functions. Postgres then keeps the `ON DELETE CASCADE` and uniqueness that prevent
orphaned check-states.

`resolve_incident_target` is the arm needing an actual decision rather than a transcription:
a cluster belongs to no group, so it has no group/rank to resolve a member target through
the way an application or machine does.

Consequence for the relay path: `ingest::Placement::Cluster` currently maps to
`Scope::Global` with the cluster as an instance label. Once `Scope::Cluster` exists it should
map there instead. Worth being deliberate about, because it changes where a cluster's checks
are read — on the cluster itself, rather than presented on its applications the way a
machine's are (CHK, "A machine's checks present on its applications").

## Cluster reachability is the ordinary rule, not a self-alert

**Revised by upstream.** The earlier design here had a Canopy-wide connectivity check with
each cluster an instance, reported as a self-alert. K8S no longer says that. Cluster
reachability now follows CHK's ordinary rule: a cluster is reachable while its relay is
reporting, and its applications while the relay reports them, with nothing deriving one
grain's reachability from another's.

That machinery already exists and is driven by **when each expected source last reported** —
that is, by filings landing — not by a bespoke liveness column. So a cluster's ongoing health
falls out of `resolve` working and filings landing against the cluster, and needs no
mechanism of its own.

## Liveness for registration reaches the page through the database

**Decided, and narrowed by the above.** The registry is in-memory in `relayhub`; the settings
page is served by `private-server`. Those are separate Kubernetes `Deployment`s, and
Canopy's established way for one pod to learn what another observed is the database, not a
pod-to-pod call.

So relayhub records what it observed of each relay, and registration reads it rather than
probing live. The private-server never talks to the relayhub.

- **Rejected: an internal HTTP surface on the relayhub.** It buys exactness registration does
  not need and introduces a pod-to-pod call pattern the codebase does not have.
- Accepted cost: "answering" becomes "answered moments ago". At registration the operator has
  just deployed the relay, so the answer is fresh by construction.

Now that reachability covers the ongoing case, this mechanism serves registration alone,
which makes its shape an open question rather than a settled one — see below.

## Open decisions to work

1. **What registration actually reads.** Since reachability no longer needs a liveness table,
   the options narrow: a small `last_answered_at` on the cluster row written by a relayhub
   probe loop, or no probe loop at all — relayhub simply records connect/disconnect, and
   registration reads "a relay is currently connected". The second is less machinery and may
   be enough, given `Ping` is answered below the `Duties` trait and a connected relay that
   cannot answer it is close to a contradiction.
2. **The enrol-then-confirm flow / UX** — registering enrols the relay, but it can only
   answer after the operator has taken the minted key, deployed it, and it has dialled in.
   How the settings page spans that, and what happens to a minted relay identity if the
   operator abandons the flow.
3. **Card size.** The model change (table, host column, `Scope::Cluster`, `resolve`) is what
   unblocks every other card; the settings page is operator-facing work nothing waits on.
   The card description already flags this as the split worth making if it runs long.

## Upstream changes to carry (from the rebase onto main)

- `NamespaceRoster` has been **removed** from `Request`/`Response`. The identity picker it fed
  needs rechecking against whatever replaced it before this card assumes it exists.
- Vocabulary reset throughout: servers → **applications**, devices → **identities** in the
  specs. The protocol's sleep/wake pair now acts on an **environment** — a group's
  applications at one rank — and AGENTS.md now forbids the older word outright, so a
  `grep -rin` for it should turn up only the `billing.*` cost-allocation label and Kubernetes
  `Deployment` resources.
