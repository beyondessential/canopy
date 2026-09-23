## Description

How much of the cluster's workload is running: the share of desired replicas that are ready, summed across Deployments, StatefulSets, DaemonSets and CNPG database clusters. Workloads scaled to zero, hibernated database clusters and Jobs are left out, so a sleeping environment contributes nothing.

A large share of everything failing to run has causes no single application's check describes: the cluster out of capacity, nodes that cannot be provisioned, a broken node class, an admission webhook refusing pods.

## Results

- **pass**: 90% or more is ready.
- **warn**: under 90% for 5 minutes.
- **fail**: under 80% for 5 minutes, under 70% for 2 minutes, or under 50% at once.

Recovering takes 5 points more than falling: a warning passes again above 95%, and a failure lifts to a warning above 85%. The share is in the detail as `healthy_share`, so a policy rule on `check.healthy_share` regrades it.

- **broken**: the relay is not permitted to read one of the workload kinds. The detail names what was refused.

## Solve

Find what is not ready with `kubectl get pods -A --field-selector=status.phase!=Running` and look for a common cause: pending pods waiting on capacity (check the node pools and Karpenter), pods failing to be created (check quotas and admission webhooks), or a node that has gone.
