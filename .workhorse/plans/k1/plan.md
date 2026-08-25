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

## Open decisions to work

1. **The `clusters` data model** — what a cluster row is, and how it references its relay
   device and is referenced by servers (L1's columns) and by `resolve`.
2. **The cross-process "connected and answering" test** — the registry is in the relayhub's
   memory; the settings page is served by the private-server. How registration confirms a
   live relay across that process boundary, and whether the per-cluster connectivity
   self-alert (K8S "When Canopy cannot read a cluster") is served the same way.
3. **The enrol-then-confirm flow / UX** — registering enrols the relay, but "connected and
   answering" can only pass after the operator has taken the minted key, deployed the relay,
   and it has dialled in. How the settings page presents that inherent two-step.
4. **Scope: does K1 build the connectivity self-alert (SELF/K8S), or is that a follow-up?**
   It shares mechanism 2 and needs the `clusters` table this card adds.
