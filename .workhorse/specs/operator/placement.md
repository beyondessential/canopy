---
id: PLACE
---

# Pod placement defaults

The restore operator stamps cluster-wide scheduling defaults onto every pod it creates: restore Jobs, replica Deployments, and the other Jobs that run against a restore.
They belong to the cluster the operator runs in rather than to any replica, so they are configured on the operator's ConfigMap.

## Configuration

The ConfigMap carries three keys, each a comma-separated list.
`nodeSelector` holds `key=value` node selector entries.
`podAnnotations` holds `key=value` pod annotations, and a value may itself contain `=`.
`tolerations` holds tolerations, each `key=value:Effect` to tolerate a taint with that exact value, or `key:Effect` to tolerate the key with any value, where the effect is `NoSchedule`, `PreferNoSchedule` or `NoExecute`.
A malformed entry is logged and skipped, and the remaining entries of its key still apply.
A change to the ConfigMap applies to pods created afterwards, without restarting the operator.

## Application

Every pod the operator creates carries the configured node selector entries, annotations and tolerations.
A node selector or annotation key the operator already set on a pod for its own reasons keeps its value, and a default only fills keys that are absent.
A configured toleration is added to a pod unless the pod already carries the same one.
