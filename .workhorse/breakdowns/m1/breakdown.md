# Substrate checks, spun off from the plumbing

M1 takes the plumbing end to end — reserving the `kubernetes` source, the instance label on the wire, the relay's watch-hold-file-refile loop, cluster-grain ingest and grading, cluster reachability, and the cluster detail page — plus three checks to prove it: the Tailscale Kubernetes API proxy, the aggregate on workloads broadly unscheduled or failing, and node pools. Their shapes differ, so between them they exercise watching one object, watching many and deriving a proportion, and filing several instances of one condition.

These are what follows. The area cards are repetitions of a pattern M1 sets, so they can run in parallel once it lands; the application grains card is gated on N1 instead.

## Cluster checks: traffic

Envoy Gateway, ingress-nginx, the AWS Load Balancer Controller and external-dns, each detected its own way, plus Gateway API drift.

Drift is the odd one and the most valuable: the installed Gateway API CRD bundle against what the Envoy Gateway controller needs. Helm installs a chart's CRDs once and never touches them on upgrade, so bumping the controller alone leaves them behind and the new controller crash-loops on a kind it gained. The ops repo documents this and refuses to deploy into it, which covers the deploy-time case and leaves a cluster that drifts afterwards unwatched.

Note this area is application-facing traffic only. Cluster operator access runs over Tailscale, not through any ingress, so a failure here is an outage for the applications' own users and says nothing about whether anyone can get in to fix it.

## Cluster checks: databases and certificates

The CNPG operator and its barman-cloud plugin, and cert-manager.

Every application has a Postgres instance CNPG reconciles, so a wedged operator is invisible until something needs it to act — a failover, a backup, a restart. Certificate renewal is silent until it isn't, and a cluster-wide cert-manager failure expires everything at once on its own schedule, which is the condition most worth warning about weeks early.

## Cluster checks: capacity

The Karpenter controller; node health; EC2 node class validity; and Karpenter's spot interruption feed. Node pools are not here — they moved into M1 as the proof check for the instance path.

Node health reads what the EKS node monitoring agent already concluded and publishes as node conditions, rather than deriving readiness afresh from the node objects. It covers memory and ephemeral disk pressure, and nodes alive but not functioning — the last graded as a warning rather than a failure, Karpenter having made it mostly a thing that gets replaced.

Node class validity is a separate condition from a pool being unhealthy: the pool is willing and the class it references cannot launch. The ops repo keeps them as separate objects, so they are separate checks against separate kinds.

The spot feed is its own too. Without the interruption feed Karpenter cannot drain a node before AWS reclaims it, so the consequence is abrupt pod loss rather than slower provisioning, and nothing else reports it.

## Cluster checks: EKS addons

CoreDNS, kube-proxy, VPC CNI, EBS CSI with its snapshot controller, EFS CSI and the Mountpoint-S3 CSI driver — the cluster's floor, where a failure is total rather than partial.

Kubernetes version support belongs here too: the EKS version approaching end of standard support, which is slow enough that months of warning is the whole value and otherwise surfaces as AWS charging extended-support rates.

## Cluster checks: platform extras

HNC, py-kube-downscaler, opencost, Prometheus, and the Tailscale operator beyond the API proxy M1 covers.

These degrade rather than break, so they are worth having and worth grading below the rest. Note py-kube-downscaler is what carries putting an environment to sleep, so its health bears on an operator action rather than only on observability.

## CNPG disk headroom, and bumping it

A CNPG cluster running out of disk space, warned about early enough to act.

This is one application's Postgres rather than a cluster-wide condition — a namespace holds one instance per central and per facility — so it sits at the application grain and not with the cluster checks.

It is its own card rather than part of the application grains because the valuable half is not a check: a way for Canopy to bump a CNPG cluster's disk before it becomes a problem. That is an action on a cluster, alongside sleeping and waking an environment, and it wants the same treatment — gated by what the environment is, available to admins, audited.

## Substrate checks for applications and namespaces

Resolve the two `FilingTarget` variants `resolve` currently places nothing for: a namespace, which is a group at a rank and files at `Scope::Group`, and an instance, which names one application scheduled in the cluster.

The substantive piece is correlating a namespace name to the group it names, for which no storage exists anywhere today — not on `server_groups`, not on `applications`, not on `kubernetes_clusters`. The approach is open between the relay reporting group and rank alongside the namespace, an operator-held mapping in Canopy, and deriving it from the namespace string by convention; the choice is bound up with the shape N1 settles for a cluster-hosted application, so it belongs with this work rather than ahead of it.

The checks themselves are alertd's, not this repo's: per application, that its workloads can be placed, no pod of it being unschedulable, and that its volumes are bound. This card is Canopy's side — placing what alertd files. The instance label M1 adds to the wire is what lets several unschedulable pods on one application be instances of one check.

Depends on N1.
