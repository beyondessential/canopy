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

**Decided: a `last_answered_at` on the cluster row**, written by relayhub, read by
registration. The private-server never talks to the relayhub.

- **Relayhub runs a probe loop**, `Ping`ing each connection it holds on a cadence and
  stamping `last_answered_at` on the cluster its relay identity resolves to. A relay whose
  identity resolves to no cluster row has nowhere to write, which is a log line rather than
  an error: the operator deleted the draft, or never finished one.
- **Stamp it on connect too, from the `Build` round trip relayhub already makes.** That
  exchange is itself proof the relay is answering, so using it means an operator who has just
  deployed a relay sees the wizard confirm almost at once, instead of waiting up to a full
  cadence for the first probe. Without this the draft flow feels broken precisely when it is
  working.
- **The freshness window registration reads against must exceed the probe cadence**, or a
  relay answering normally reads as stale between probes. Both are knobs to pick with a real
  relay in front of us rather than guessed here.
- **Disconnect does not clear the column.** It is a timestamp and staleness is computed from
  it, so clearing would discard the "when did we last hear from this" an operator wants when
  diagnosing a cluster that has gone quiet.

**Rejected: an internal HTTP surface on the relayhub** for a live probe. It buys exactness
registration does not need and introduces a pod-to-pod call pattern the codebase does not
have.

Accepted cost: "answering" means "answered moments ago". Since reachability covers the
ongoing case, this column serves registration and operator display, and nothing grades on
it — so the imprecision has nowhere to do harm.

## One wizard, and a draft is the trace it leaves

**Decided.** Registration is a single wizard rather than two independent steps, because a
cluster registration page that does not mint the credential sends operators hunting for the
identity page. The cost of one wizard is that the relay identity is minted partway through,
so an abandoned wizard must not leave that identity orphaned.

So the `kubernetes_clusters` row is written when the credential is minted, in a **draft**
state, carrying the name and the `relay_identity_id`. The minted relay is therefore never
loose: the draft row is what says which cluster it was for and who minted it.

- **Draft is a nullable `registered_at`**, matching how `applications.registered_at` already
  carries "created but not yet reported". Null is a draft; set is a registered cluster.
- **`registered_at` is set when `Ping` succeeds**, and only then.
- **Everything downstream reads registered clusters only** — the host picker, an
  application's `kubernetes_cluster_id`, `Scope::Cluster`, and `resolve`. A draft is a
  registration in progress, not a cluster in the registry.

This keeps what the spec is actually protecting. K8S's "before the cluster is saved" exists
so that "a cluster Canopy cannot read is caught as the operator adds it"; a draft is visibly
not yet registered, so nothing unconfirmed enters the registry or acquires applications.

### Drafts are resumable, which is what makes them worth keeping

A draft that could not be resumed would be litter with a name on it, since the private key is
shown once and is gone with the abandoned wizard. It can be resumed:
`provision_credential` takes an **optional `device_id`** and mints through
`DeviceKey::create`, which — unlike `Device::add_key`, the enrolment path that refuses a
second active key — simply inserts another. So "re-issue credential" on a draft works with
the existing endpoint.

Deactivate the superseded key when re-issuing. The draft's previous key was never deployed
anywhere, so retiring it costs nothing and keeps a draft from accumulating active keys.

**Decided: drafts never expire.** No sweep, no age-out — a draft is removed by an operator
or not at all. A draft that vanished on a timer would take its relay identity's only trace
with it, which is the precise thing the draft exists to prevent, and there will never be
enough of them for tidiness to outweigh that.

### Spec impact to carry back

A draft is product-visible behaviour an operator sees and acts on, so **K8S's "Cluster
registry" wants a sentence or two for it** — that registering mints the relay's credential,
that an unfinished registration is kept as a draft rather than discarded, and that a draft
holds no applications. Worth drafting once the wizard's shape is agreed, not before.

## The card stays whole

**Decided: no split.** The card description floated separating the model change from the
settings page, and the model half did grow once the September reset landed. Building both
here anyway: the two halves share the draft-state design, which straddles them — the draft
row is a model concern and the wizard is what creates and resumes one — so splitting would
put one design across two cards and leave the first unable to demonstrate the behaviour it
exists for.

### Build order, since nothing is now waiting on a decision

Each step is usable before the next begins, so the card can be reviewed in pieces even
though it lands as one.

1. **`kubernetes_clusters`** — `id`, `name`, `relay_identity_id`, nullable `registered_at`,
   nullable `last_answered_at`. Everything below needs the table.
2. **A cluster is a host** — `applications.machine_id` becomes nullable, a
   `kubernetes_cluster_id` joins it, and a CHECK holds exactly one. The largest migration of
   the card, and the one most likely to surface call sites assuming a machine is always
   present.
3. **`Scope::Cluster`** — variant, column, and an arm in each of `to_columns`,
   `from_columns`, and `resolve_incident_target`, per the AGENTS.md recipe.
4. **`resolve`** — with a cluster to resolve against and a scope to file at, the seam J2 left
   can finally place a filing. This is the step that makes the whole relay path live.
5. **Relayhub probe loop** — periodic `Ping`, plus the stamp on connect from the existing
   `Build` exchange.
6. **The settings page** — the wizard, the draft list, and re-issuing a draft's credential.

Step 4 is where the project stops being plumbing: every card downstream is unblocked the
moment a filing can land, and steps 5 and 6 are what make a cluster registrable by an
operator rather than by a migration.

### Implementation checklist

- [x] 1a. Migration: `kubernetes_clusters` table (`id`, `name`, `relay_identity_id` unique, nullable `registered_at`, nullable `last_answered_at`)
- [x] 1b. `KubernetesCluster` model — create draft, register (set `registered_at`), list registered / list drafts, rename, remove, stamp `last_answered_at`, `names_by_ids`
- [x] 2a. Migration: `applications.machine_id` nullable, add `kubernetes_cluster_id`, host CHECK (exactly one); guard the machine-group trigger for cluster apps
- [x] 2b. `Application` model + call sites tolerate a null machine / cluster host
- [x] 3a. Migration: add `kubernetes_cluster_id` to the scoped tables (issues, scoped silences) + widen their CHECKs + THE TRAP global indexes
- [x] 3b. `Scope::Cluster` variant + arm in `to_columns`, `from_columns`, `resolve_incident_target`, batch resolve, filing dispatch, `raise_cluster_event_with_state`, `FilingScope`/`scoped_to`, MCP `IssueScopeOut`
- [x] 4. `jobs::relay::ingest::resolve` — relay identity → registered cluster; `Cluster`→`Scope::Cluster`. `Namespace`→group and `Instance`→application await the harvest correlation path (logged unplaceable until then)
- [x] 5. Relayhub probe loop — periodic `Ping` + stamp `last_answered_at` on connect from the `Build` round trip
- [x] 6a. Private-server fns (`/api/kubernetes_clusters/*`): register (mint + draft), confirm (register on answer), re-issue (retiring the superseded key), remove, list. Shared `mint_provisioned_credential` extracted from `provision_credential`. Backend integration tests pass
- [x] 6b. React settings page (Settings › Clusters): register wizard with credential reveal + live connection polling, draft list with re-issue/remove/check, registered list. Typechecks
- [x] 6c. Playwright coverage (`e2e/kubernetes-clusters.spec.ts`, 4 tests) — register→draft, registered-shows-answering, remove, empty-name refused
- [x] 7. Spec impact: K8S's "Cluster registry" already describes drafts, re-issue, and "only a registered cluster hosts applications"; CHK/FLT already describe cluster-as-host and cluster-as-check-target. This card implements existing specs (verifies K8S/CHK/FLT); no spec change needed

**Not done here (host picker):** offering a registered cluster as an application's host in the create/edit UI is part of the operator-facing application-on-a-cluster surface, which arrives with the harvest card that creates cluster-hosted applications. Registered clusters are already exposed by `/api/kubernetes_clusters/list` for that work to build on.

**Deferred to the operator-facing / harvest work (noted, not silently dropped):**
- Cluster-hosted applications are not created by this card (the harvest path is unwired), so the machine-oriented fleet surfaces (group card, fleet spread, `ServerInfo` listings) list machine-hosted applications only. Presenting cluster-hosted applications there arrives with the harvest card that creates them.
- `resolve` for `Namespace`/`Instance` targets awaits namespace→group and instance→application correlation, which the harvest path brings.

## Upstream changes to carry (from the rebase onto main)

- `NamespaceRoster` has been **removed** from `Request`/`Response`. The identity picker it fed
  needs rechecking against whatever replaced it before this card assumes it exists.
- Vocabulary reset throughout: servers → **applications**, devices → **identities** in the
  specs. The protocol's sleep/wake pair now acts on an **environment** — a group's
  applications at one rank — and AGENTS.md now forbids the older word outright, so a
  `grep -rin` for it should turn up only the `billing.*` cost-allocation label and Kubernetes
  `Deployment` resources.
