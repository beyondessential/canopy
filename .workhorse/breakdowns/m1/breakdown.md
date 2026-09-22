# Substrate checks: the application-facing grains

M1 keeps Canopy's connection to a cluster and the cluster's own health, which depend on nothing outstanding.
The grains that describe what a cluster does with one application's workloads spin off here, because they cannot be built or tested until N1 creates a cluster-hosted application to file against.

## Substrate checks for applications and namespaces

Resolve the two `FilingTarget` variants `resolve` currently places nothing for: a namespace, which is a group at a rank and files at `Scope::Group`, and an instance, which names one application scheduled in the cluster.

The substantive piece is correlating a namespace name to the group it names, for which no storage exists anywhere today — not on `server_groups`, not on `applications`, not on `kubernetes_clusters`.
The approach is open between the relay reporting group and rank alongside the namespace, an operator-held mapping in Canopy, and deriving it from the namespace string by convention; the choice is bound up with the shape N1 settles for a cluster-hosted application, so it belongs with this work rather than ahead of it.

Per application, the relay determines that the application's workloads can be placed, no pod of it being unschedulable, and that its volumes are bound, tolerating a namespace still on `Ingress` rather than `Gateway` and not alarming on a duty deliberately scaled to zero.

Depends on N1.
