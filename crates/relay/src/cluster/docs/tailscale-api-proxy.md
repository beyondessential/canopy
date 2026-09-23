## Description

Whether operators can get into the cluster's API over the tailnet, through the Tailscale operator's Kubernetes API proxy. Operator access does not go through the ingress that serves the applications, so a cluster can serve its users while nobody can get in to work on it.

The proxy runs one of two ways. As a ProxyGroup of type `kube-apiserver`, the operator reports both whether a proxy is ready to serve and the tailnet devices it holds, and the check reads both. Run in-process inside the operator, nothing reports the operator's own tailnet connection, so the check reads only whether the operator is ready. The detail's `mode` says which was read.

## Results

- **pass**: the proxy is ready and, where it can be read, connected to the tailnet.
- **fail**: the proxy is not ready, or it is ready but not connected to the tailnet. The detail's `ready` and `connected` say which.
- **broken**: the relay is not permitted to read ProxyGroups or Deployments. The detail names what was refused.

## Solve

For a ProxyGroup, `kubectl describe proxygroup <name>` shows its conditions and devices. For the in-process proxy, check the operator's pod and logs in its namespace. A proxy that is ready but off the tailnet usually means its tailnet auth has lapsed: check the operator's OAuth client or workload identity, and the device in the Tailscale admin console.
