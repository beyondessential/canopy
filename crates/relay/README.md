# relay

The Canopy relay: one per Kubernetes cluster, opening its connection outward to Canopy and filing the cluster's checks.
See the Kubernetes monitoring spec (`.workhorse/specs/monitoring/kubernetes.md`) for what it does.

## Permissions

The relay reads the cluster through its own ServiceAccount.
The cluster checks need only to list and watch the kinds they read, cluster-wide:

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: canopy-relay
rules:
  # workloads-running, and tailscale-api-proxy for an in-process proxy
  - apiGroups: ["apps"]
    resources: ["deployments", "statefulsets", "daemonsets"]
    verbs: ["list", "watch"]
  - apiGroups: ["postgresql.cnpg.io"]
    resources: ["clusters"]
    verbs: ["list", "watch"]
  # node-pools
  - apiGroups: ["karpenter.sh"]
    resources: ["nodepools"]
    verbs: ["list", "watch"]
  # tailscale-api-proxy, for a proxy run as a ProxyGroup
  - apiGroups: ["tailscale.com"]
    resources: ["proxygroups"]
    verbs: ["list", "watch"]
```

Bind it to the relay's ServiceAccount with a ClusterRoleBinding.
A permission missing from this set does not stop the relay: the check that needed it files as broken, naming the verb and resource refused, until it is granted.
A kind whose CRD is not installed is not a refusal: a cluster without Karpenter files no `node-pools` check, one without CNPG counts no database clusters, and one with no Tailscale API proxy, in either form, files no `tailscale-api-proxy` check.
