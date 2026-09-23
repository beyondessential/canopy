## Description

One instance per Karpenter node pool in the cluster, graded on the pool's own conditions as Karpenter reports them. A pool that cannot provision nodes leaves whatever is scheduled to it pending.

## Results

- **pass**: the pool is ready and the nodes it launches register with the cluster. A pool that is not ready only because its node class is not ready passes here, the node class being its own condition.
- **fail**: the pool is not ready, or the nodes it launches are not registering. The detail names the condition and what Karpenter said about it.
- **broken**: the relay is not permitted to read node pools. The detail names what was refused.

## Solve

Read the pool's conditions with `kubectl describe nodepool <pool>`. Nodes not registering usually means they launch but cannot join: check the node class's AMI, subnets, security groups and instance profile, and the Karpenter controller's logs.
