# Cluster registry and connection — test cases

Concrete scenarios that verify the in-app cluster registry, a cluster as a host
and a check target, cluster reachability, the relay ingest `resolve` seam, and
the registration wizard. Ticked cases are covered (by an automated test or a
manual run); unticked cases are coverage still owed.

## The `kubernetes_clusters` table and model

- [ ] A cluster row holds a name and a `relay_identity_id`, with `relay_identity_id` unique — a second cluster cannot reference the same relay identity (verifies spec: K8S)
- [x] A draft cluster has `registered_at` null; a registered cluster has it set (verifies spec: K8S)
- [ ] Deleting the relay identity removes its cluster row (cascade), so no cluster is left without its identity
- [x] Registered-cluster listing excludes drafts; the draft listing shows only drafts (verifies spec: K8S)

## A cluster is a host

- [ ] An application can be created on a cluster with no machine, and reports/presents no machine facts (verifies spec: FLT, K8S)
- [ ] An application on a machine has `machine_id` set and `kubernetes_cluster_id` null; on a cluster, the reverse (verifies spec: FLT)
- [ ] The host CHECK rejects an application with both a machine and a cluster, and one with neither (verifies spec: FLT)
- [ ] An application on a cluster takes its group from its namespace, not from an operator; the cluster itself belongs to no group (verifies spec: K8S, FLT)
- [ ] Existing call sites that read an application's machine tolerate a cluster-hosted application (no unwrap/500 on a null machine)

## A cluster is a check target

- [x] `Scope::Cluster(id)` round-trips through `to_columns`/`from_columns` (verifies spec: CHK)
- [ ] A check filed at `Scope::Cluster` lands on the cluster and is read there, not presented on the cluster's applications (verifies spec: CHK, K8S)
- [ ] The scoped-table CHECK admits a cluster-scoped row and still forbids two grains at once
- [ ] A cluster-scoped check can be silenced at the cluster scope (verifies spec: CHK)
- [x] `resolve_incident_target` for a cluster yields no group-member incident target, a cluster belonging to no group (verifies spec: CHK)

## Cluster reachability

- [ ] A cluster is reachable while its relay is reporting, and unreachable when the relay stops (ordinary reachability rule, no self-alert) (verifies spec: CHK, K8S)
- [ ] An application on a cluster is reachable while the relay reports it, and becomes unreachable on its own account when the relay goes quiet (verifies spec: CHK)

## The relay ingest `resolve` seam

- [ ] A substrate `Cluster` filing from a registered cluster's relay resolves to `Scope::Cluster` and lands (verifies spec: K8S)
- [ ] A filing from a relay whose identity resolves to no registered cluster (draft or absent) is logged as unplaceable, not filed
- [ ] A `Namespace` filing resolves to the group its namespace names (verifies spec: K8S)
- [ ] A filing from a draft cluster's relay is not placed until the draft becomes registered

## Registration wizard and drafts

- [x] Registering names the cluster and mints the relay credential, returning the private key once (verifies spec: K8S, DPK)
- [x] Minting writes the cluster as a draft (name + relay identity), so an abandoned wizard leaves a record of what the identity was for (verifies spec: K8S)
- [x] The cluster is saved as registered only once its relay connects and answers a Ping (verifies spec: K8S)
- [x] Re-issuing a draft's credential retires the previous key and returns a new private key (verifies spec: K8S)
- [ ] A draft persists until an operator removes it; nothing ages it out (verifies spec: K8S)
- [x] A relay that answers Ping while its cluster access is incomplete still registers (registration turns on the connection, not on RBAC) (verifies spec: K8S)

## Liveness for registration

- [ ] The relay hub stamps `last_answered_at` on connect (from the Build round trip) and on a periodic Ping probe (verifies spec: K8S)
- [ ] Registration reads a cluster as answering when `last_answered_at` is within the freshness window
- [ ] A relay whose identity resolves to no cluster row is logged, not errored, when the probe finds nowhere to write
- [ ] Disconnect does not clear `last_answered_at`

## Settings page (Playwright)

- [x] An admin registers a cluster through the wizard end to end and sees it confirmed once the relay answers
- [x] The draft list shows an unfinished registration and offers re-issue and removal
- [ ] A registered cluster is offered as an application's host
