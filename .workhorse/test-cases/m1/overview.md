# Test cases: substrate checks for the cluster grain

## The source and the cluster grain

- [x] A substrate filing at cluster grain from a registered cluster's relay lands as one state at the cluster, its check flat rather than qualified by an application type (verifies spec: K8S, CHK)
- [x] A status push over the device API claiming the `kubernetes` source is refused, whatever its casing (verifies spec: CHK)
- [x] The Sources page doesn't list `kubernetes`, and setting a reachability or ingest mode on it is refused (verifies spec: CHK)
- [x] A substrate check registers already reviewed at the ceiling and escalation its filing named, and a later filing doesn't overwrite an operator's edited policy (verifies spec: K8S)
- [ ] A substrate filing from a relay whose cluster is still a draft places nothing and is logged (verifies spec: K8S)
- [x] A cluster-scoped issue that fails opens no incident, and counts towards the cluster's health (verifies spec: INC)

## Cluster reachability

- [x] A registered cluster whose relay stops filing reads unreachable once past its threshold, and recovers when filings resume (verifies spec: CHK, K8S)
- [x] A registered cluster that has never been filed against presents as never reported, not unreachable (verifies spec: CHK)
- [x] A draft cluster is swept for nothing and presents no reachability (verifies spec: K8S)
- [x] A new cluster's threshold defaults to five minutes, and an admin changing it on the cluster's page changes when the cluster reads unreachable (verifies spec: K8S)

## The cluster's page

- [x] The page presents the cluster's health, reachability and checks, and lists the applications it hosts (verifies spec: K8S)
- [x] The registry links each registered cluster to its page (verifies spec: K8S)
- [x] Silencing a cluster check on the page quiets it on the cluster, and no group-scoped silence is offered (verifies spec: CHK)
- [x] A non-admin can read the page but can't edit the threshold or silence checks (verifies spec: K8S)
- [x] Playwright spec covering the page with a seeded cluster and cluster-scoped issues

## The relay's filing loop

- [x] A cluster condition changing files promptly, not on the next refile (verifies spec: K8S)
- [ ] An unchanged condition refiles every minute, so a quiet, healthy cluster stays reachable (verifies spec: K8S, CHK)
- [ ] A filing attempted while the connection is down isn't lost to it: the next refile carries the current state (verifies spec: K8S)
- [ ] A permission the relay hasn't been granted makes the check that needed it broken, naming what was refused, and the check is never absent (verifies spec: K8S)

## Instances

- [x] A check with instances files all of them in one filing, and Canopy holds one state with each instance graded on its own policy (verifies spec: CHK, K8S)
- [x] The effective result is the most urgent instance, and the detail lists every instance not passing (verifies spec: CHK)
- [ ] A silence on one node pool quiets only that pool (verifies spec: CHK)
- [x] The relay-protocol round trip carries a multi-instance substrate filing unchanged

## Tailscale API proxy

- [x] A `kube-apiserver` ProxyGroup passes when a proxy is available and it holds a tailnet device (verifies spec: K8S)
- [x] A ProxyGroup fails when no proxy is available, and fails when available but holding no tailnet device, the detail saying which (verifies spec: K8S)
- [x] A proxy run in-process in the operator is graded on the operator's readiness alone, the detail marking the tailnet half as not read (verifies spec: K8S)
- [x] A ProxyGroup is read in preference to the operator where both exist (verifies spec: K8S)
- [x] A cluster with no API proxy in either form files no check (verifies spec: K8S)
- [ ] On a cluster from the ops repo, the in-process operator is found by its `APISERVER_PROXY` env and the check passes

## Node pools

- [x] Each NodePool files as its own instance (verifies spec: K8S)
- [x] A pool that isn't ready fails; a pool whose launched nodes don't register fails (verifies spec: K8S)
- [x] A pool not ready only because its node class isn't ready isn't failed on that account (verifies spec: K8S)

## Workloads broadly not running

- [x] The share is ready over desired replicas across Deployments, StatefulSets, DaemonSets and database clusters (verifies spec: K8S)
- [x] A Deployment whose pods can't be created (quota exceeded, admission webhook down) counts against the share even though none of its pods exist (verifies spec: K8S)
- [x] Workloads scaled to zero, by an operator or by the downscaler, add nothing to either side (verifies spec: K8S)
- [x] A hibernated database cluster adds nothing to either side (verifies spec: K8S)
- [ ] Jobs and CronJobs aren't counted (verifies spec: K8S)
- [x] A sleeping environment, with its Deployments scaled down and its database clusters hibernated, adds nothing, and putting it to sleep doesn't move the check (verifies spec: K8S)
- [x] Bands: 89% held 5 minutes warns; 79% held 5 minutes fails; 65% held 2 minutes fails; 45% fails at once (verifies spec: K8S)
- [x] Hysteresis: a warning at 92% stays a warning and passes at 96%; a failure at 83% stays failed and lifts to a warning at 86% (verifies spec: K8S)
- [x] A dip below 90% that recovers inside 5 minutes files nothing degraded (verifies spec: K8S)
- [x] Recovery takes effect as soon as the share crosses back over its edge (verifies spec: K8S)
- [ ] The detail carries the share as a number, and a catalog rule on it regrades the check (verifies spec: K8S, CHK)
- [x] After a relay restart at 85%, the check files nothing until its hold has passed, and the open issue stays open rather than recovering and reopening (verifies spec: K8S)
- [x] After a relay restart at 97% or 40%, the check files at once (verifies spec: K8S)

- [ ] A watch the cluster refuses with a 403 is classified as a refusal of that verb and resource, and a kind whose CRD is absent is not
- [ ] A watch failing for under two minutes keeps the check determined from what it last held; past that the check is broken

## Operational

- [ ] Observe the aggregate on the smallest cluster before relying on it: across a scheduled sleep, a wake (many pods pending while Karpenter provisions nodes), and a cluster upgrade. Adjust the defaults if the bands are wrong in practice
