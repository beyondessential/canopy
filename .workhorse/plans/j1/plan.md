# Tamanu Kubernetes namespace layout — research spike (J1)

Output of the J1 spike. Maps every input Canopy's Kubernetes checks and setup need
onto where it actually lives in a Tamanu namespace, and how it is named or labelled,
so L1 (identity picker), M1 (`kubernetes` infra checks) and N1 (`alertd` harvest) can
query it reliably. The behaviour these feed is specified in `monitoring/kubernetes.md`
(K8S); this document is the concrete reference, not a spec.

## Sources

The Tamanu k8s deployment is not defined in the Tamanu app repo. It is the Pulumi
stack `tamanu/on-k8s` in `beyondessential/ops` (`pulumi/tamanu/on-k8s/`), driven by
deploy options assembled in Tamanu's `packages/scripts/src/ghaCdHelpers.mjs`
(`configMap()`). Every naming and labelling fact below is read from that stack's
source, cross-checked against the operator docs in the Tamanu repo
(`llm/docs/database-backups.md`, `docs/sops/connect-psql.md`) and the ops k8s specs
(`docs/spec/kubernetes/`). CNPG-standard behaviour (PVC and pod naming, role labels)
is called out where it comes from the CloudNativePG operator rather than the stack.

## The namespace and what a "server" is inside it

One namespace holds one deploy: a server group at one rank. The namespace is
`tamanu-<deploy-name>` (`src/namespace.ts`, `configMap()` sets `namespace`). It is
created externally (an HNC `SubnamespaceAnchor` under `tamanu-super`), so Canopy
should treat the namespace as pre-existing and never assume it owns it.

A namespace contains exactly one central and zero or more facilities. Each is a
distinct server in Canopy's model, and every resource belonging to one carries a
**namespace-local name prefix** that is the join key tying that server's database,
workloads and gateway together:

- **central** → prefix `central`
- **facility N** → prefix `facility-<N>`, where **N is a positional index (1, 2, …),
  not the facility's id or name** (`src/index.ts`: `new FacilityServer(\`${i + 1}\`, …)`).

This positional prefix is the single most important finding. A facility's Tamanu
identity (its facility id, which doubles as its device id, and its subdomain host)
does **not** appear in the names of its database, PVCs, or CNPG secret. It appears
only in:

- the `app.kubernetes.io/instance` **label** on the facility's app workloads (api,
  sync, tasks, fhir), whose value is the facility id, and
- the **Gateway listener hostname** (the `host` subdomain), and
- the facility's ConfigMap contents (`config-facility-<N>`).

So Canopy cannot read a facility's id off its database or PVC names. It must join:
CNPG cluster `facility-<N>-db`  ↔  app workloads `facility-<N>-*` (which carry
`app.kubernetes.io/instance: <facilityId>`)  ↔  Gateway `facility-<N>` (whose
listener hostname is the facility's URL). The shared `facility-<N>` prefix is what
makes the join reliable; the id/host is then recovered from the workload label or
the gateway hostname.

Central carries no `app.kubernetes.io/instance` label; it is identified by
`app.kubernetes.io/name: central` alone.

## Common labelling scheme

App workloads (not the CNPG database) follow the k8s recommended labels
(`src/central/specs.ts`, `src/facility/specs.ts`, `src/web/specs.ts`):

| label | central | facility |
|---|---|---|
| `app.kubernetes.io/name` | `central` | `facility` |
| `app.kubernetes.io/component` | the duty (see below) | the duty |
| `app.kubernetes.io/instance` | (absent) | the facility id / device id |
| `app.kubernetes.io/version` | image tag | image tag |

Note the asymmetry: on **app workloads** the name label is the role (`central` /
`facility`) and the facility id is in `instance`; on the **CNPG database** (below)
the name label is the positional prefix (`central` / `facility-<N>`). They do not use
the same convention, so a query must know which kind of resource it is reading.

Every workload also carries AWS cost-allocation labels (`billingTags`:
`product=tamanu`, plus stage/deployment) via `costLabels`. Not useful for identity.

## Required inputs → where each lives

### 1. The server-identity picker (L1): list centrals and facilities in a namespace

Enumerate the CNPG clusters (`kind: Cluster`, group `postgresql.cnpg.io`) in the
namespace carrying `app.kubernetes.io/component: database`. Their names are
`central-db` and `facility-<N>-db`, giving the positional roster. For each facility,
recover its Tamanu id from the `app.kubernetes.io/instance` label on any of its app
workloads (e.g. the deployment named `facility-<N>-api` or `facility-<N>-sync`), and
its URL from the Gateway `facility-<N>` listener hostname. Central is the cluster
`central-db` / the `app.kubernetes.io/name: central` workloads.

There is no single resource that lists "the servers"; the roster is derived from the
per-server prefixes present in the namespace.

### 2. Workloads by duty (M1: readiness, crashloop/restart, resource pressure)

All are `apps/v1` Deployments in the namespace. Names and component labels:

**Central** (`app.kubernetes.io/name: central`):

| duty | Deployment name | `component` label | replicas source |
|---|---|---|---|
| central API | `central-api` | `api-server` | `centralApiReplicas` (default 2) |
| central tasks | `central-tasks` | `task-runner` | 1 if enabled, else 0 |
| central FHIR refresh | `central-fhir-refresh` | `fhir-refresh` | = tasks replicas |
| central FHIR resolver | `central-fhir-resolver` | `fhir-resolver` | = tasks replicas |

**Facility N** (`app.kubernetes.io/name: facility`, `app.kubernetes.io/instance: <facilityId>`):

| duty | Deployment name | `component` label |
|---|---|---|
| facility API | `facility-<N>-api` | `api-server` |
| facility sync | `facility-<N>-sync` | `sync-server` |
| facility tasks | `facility-<N>-tasks` | `task-runner` |
| facility FHIR refresh | `facility-<N>-fhir-refresh` | `fhir-refresh` |
| facility FHIR resolver | `facility-<N>-fhir-resolver` | `fhir-resolver` |

Readiness = the Deployment's `.status.readyReplicas` vs `.spec.replicas`. Crashloop /
restart = container restart counts and waiting reasons (`CrashLoopBackOff`) on the
pods selected by the Deployment. Resource pressure = pod/container status for
`OOMKilled` termination reasons and `Evicted` pod phase. The single app container in
each pod is named after its duty (`podType`): `api-server` pods run a container named
`server`, tasks a container named `task-runner`, fhir a container named
`fhir-refresh`/`fhir-resolver`. Container CPU/memory requests are small
(`5m`/`150–200Mi`) and only CPU/mem *requests* are set, no limits, so pressure shows
as OOM/eviction and node-level signals rather than as limit throttling.

Migrator and provisioner run as `batch/v1` Jobs (`central`/`facility-<N>` migrator,
central provisioner), not Deployments. They are one-shot and should not be read as
long-running workloads for readiness. A facility also has a one-shot
`facility-<N>-setup-sync` Job on images ≥ 2.60.

Replica counts are operator-tunable per deploy (0–5 for api/web, 0–1 for tasks), so a
duty legitimately having **zero** replicas is a valid configuration, not a fault. M1
must read expected replicas from the Deployment's own `.spec.replicas`, never assume a
fixed count, and not alarm on a duty that is deliberately scaled to zero.

### 3. Ingress / front-end API for the liveness check (M1: "server live")

Tamanu on-k8s uses the **Gateway API (Envoy)**, not `Ingress`. Per server, one
`Gateway` (`gateway.networking.k8s.io`, `gatewayClassName: envoy`) named after the
prefix: `central`, `facility-<N>`, and `patient-portal`. Its single HTTPS listener
(port 443, TLS terminate) carries the server's public **hostname**, which is the
liveness target. TLS secret `<prefix>-termination-tls`, issued by cert-manager
(`letsencrypt` ClusterIssuer).

Routing is a set of `HTTPRoute`s per server (`<prefix>-frontend` → web service,
`<prefix>-api` and `<prefix>-api-legacy` → the API service, plus import routes). The
**front-end API service** the liveness check hits behind the gateway is:

- central: Service `central-api` (port 80 → container port 3000, target port `http`)
- facility: Service `facility-<N>-api` (same shape)

The API container answers `GET /` (used as its own startup/liveness probe in-cluster),
so the same path is a sound liveness signal. Hostnames follow
`<label>.<domainBase>`: `central.<base>`, `<facilityHost>.<base>`,
`portal.<base>`, where `<base>` is `demos.tamanu.app` on the demo cluster or
`cd.tamanu.app` on the main cluster (a per-deploy `domainBase` override is possible).

Caveat: the ops cluster is mid-migration from ingress-nginx to Envoy Gateway
(`docs/spec/kubernetes/ingress.md`). tamanu-on-k8s is already on the Gateway path, so
`Gateway`/`HTTPRoute` is current, but M1 should tolerate a namespace that has not been
migrated (an `Ingress` instead) rather than treating the absence of a `Gateway` as the
server being gone.

### 4. PVCs for the storage check (M1: "storage")

Storage per server is the CNPG database's volume. CNPG creates one PVC per instance,
named after the instance pod: `<prefix>-db-<ordinal>` (e.g. `central-db-1`,
`central-db-2`, `facility-1-db-1`), labelled `cnpg.io/cluster: <prefix>-db` (standard
CNPG). The declared capacity is on the `Cluster` resource at `.spec.storage.size`
(the ops restore workflow reads exactly this field); default `5Gi`, operator-tunable
`dbStorage` up to 100Gi. There is one storage class only (block storage); shared-FS
volumes exist only if `sharedBlobStorage` is enabled, which is off by default. So the
storage check is: for each server, the PVCs labelled `cnpg.io/cluster: <prefix>-db`,
bound, and utilisation against `.spec.storage.size`.

### 5. Each server's Postgres and the CNPG-managed secret (N1: DB harvest)

Every central and facility has its own CNPG `Cluster` named `<prefix>-db`
(`central-db`, `facility-<N>-db`), group `postgresql.cnpg.io/v1`, labelled
`app.kubernetes.io/name: <prefix>`, `app.kubernetes.io/component: database`. No
database is shared between servers.

Connect via CNPG's generated Services (standard CNPG):

- `<prefix>-db-rw` — primary, read-write (what the app uses; harvest target)
- `<prefix>-db-ro` — hot standbys, read-only
- `<prefix>-db-r` — any instance

Database name is **`app`** (the CNPG default; a bare `psql` hits the wrong DB, hence
the operator SOP insists on `psql app`). Port 5432. The primary pod carries
`cnpg.io/instanceRole: primary` (the ops `central-db-tailscale` Service selects on
exactly this), so the primary is found by label, never by assuming ordinal `-1`.

**The CNPG-managed secret backing each instance** is `<prefix>-db-app` (type
`kubernetes.io/basic-auth`, keys `username` and `password`), auto-created by CNPG for
the `app` role. This is the secret N1 harvests for credentials: read
`<prefix>-db-app`, connect to `<prefix>-db-rw` database `app`. A superuser secret
`<prefix>-db-superuser` also exists (CNPG, because the stack sets
`enableSuperuserAccess: true`).

There is additionally a **stack-authored** convenience secret `<prefix>-db-url`
(Opaque, labelled like the database) holding ready-made connection strings, with key
`app.dburl` = `postgres://app:<pw>@<prefix>-db-rw/app`. Useful, but it is created by
the Pulumi stack rather than by CNPG, so N1 should treat `<prefix>-db-app` as the
canonical source and `<prefix>-db-url` as an optional shortcut only where present.

### 6. Facility name / id / prefix to locate its DBs and containers

Covered by §1 and the prefix discussion above. Concretely, given a facility the
operator has identified in the picker:

- its **resource prefix** is `facility-<N>` (positional) — use it to name its
  Deployments, Services, CNPG cluster, PVCs, secret and Gateway;
- its **facility id / device id** is the `app.kubernetes.io/instance` label value on
  its app workloads, and the value threaded into its ConfigMap;
- its **host** (URL label) is the Gateway listener hostname.

L1 should persist the mapping it needs (the prefix, plus the id/host it displays) at
identity-set time, because none of these can be reconstructed from another alone: the
prefix is not derivable from the id, and the id is not derivable from the prefix.

## Cross-check: how this maps onto the K8S spec's checks

| Check (K8S) | Namespace inputs |
|---|---|
| Server live | Gateway `<prefix>`, listener hostname; front-end API Service `<prefix>-api` answering `GET /` |
| Workloads ready | Deployments `<prefix>-*` by `component` label; `.status.readyReplicas` vs `.spec.replicas` |
| Restarts | pods of those Deployments; container restart count / `CrashLoopBackOff` |
| Database up | CNPG `Cluster <prefix>-db`; Service `<prefix>-db-rw`; DB `app` |
| Storage | PVCs labelled `cnpg.io/cluster: <prefix>-db`; capacity `Cluster.spec.storage.size` |
| Resource pressure | pods of the app Deployments; `OOMKilled` / `Evicted` |
| Reachability | the namespace `tamanu-<deploy-name>` existing at all |
| DB-harvested (alertd) | secret `<prefix>-db-app` → connect `<prefix>-db-rw`/`app` |

## Open items and caveats for the implementation cards

- **Positional facility prefix.** The single biggest gotcha: `facility-<N>` is an
  index, not the facility name. L1's picker must join across resource kinds to bind
  the prefix to the facility id/host, and persist that binding.
- **Zero-replica duties are valid.** Read expected counts from `.spec.replicas`; do
  not alarm on a deliberately-scaled-to-zero duty (M1).
- **Gateway API, not Ingress.** Query `Gateway`/`HTTPRoute`; tolerate an
  un-migrated namespace still on `Ingress` without treating it as "server gone".
- **Two labelling conventions.** App workloads label `name`=role +
  `instance`=facilityId; the CNPG database labels `name`=prefix. A query must know
  which it is reading.
- **Facilities can be renamed vs moved.** `id` and `host` are deliberately separable
  (`src/config.ts`), so do not assume a facility's URL equals its id.
- **Namespace is externally owned** (HNC subnamespace under `tamanu-super`); Canopy
  reads, never creates or reconciles it.
- **TTL hibernation.** Deploys with a TTL are scaled to zero and their CNPG clusters
  hibernated after the window (`schedule.ts` / kube-downscaler). A hibernated but
  still-present namespace is not the same as a deleted one; worth confirming how M1/N1
  should treat a hibernated deploy (likely broken/unconfirmed rather than failing),
  but that is behaviour for the source-worker cards, not a layout fact.
- **RBAC read set (feeds H1/M1/N1).** To do all the above read-only, Canopy needs, in
  each Tamanu namespace: `pods`, `services`, `persistentvolumeclaims`, `secrets`,
  `configmaps`, `apps/deployments`, `batch/jobs`, `postgresql.cnpg.io/clusters`,
  `gateway.networking.k8s.io/gateways` + `httproutes` (and `networking.k8s.io/ingresses`
  while the migration lasts), plus pod metrics for pressure. Secret read is required
  for the DB harvest and should be scoped as tightly as the cluster auth (H1) allows.
