# Explore canopy as OIDC IdP for workload identity

Work split out of the exploration so the core IdP can be shaped first. The Canopy side (discovery, JWKS, token endpoint, grants, signing key) stays on this card.

## Kubernetes workloads authenticate to Anthropic

Tamanu pods on Kubernetes have no Canopy identity, only the cluster relay does. In the first step, pods use the cluster's own OIDC issuer registered directly with Anthropic, so policy lives in Anthropic rules and no Canopy work is needed beyond documenting the setup. In the second step, the relay performs a TokenReview on a pod's projected service-account token and Canopy issues its own JWT carrying the namespace's group and rank, giving one issuer and one policy and audit surface across machines and pods.

## bestool serves identity tokens locally

A workload on an enrolled box obtains a Canopy-issued identity token by calling a local HTTP endpoint on bestool, which holds the device key and calls Canopy on the workload's behalf. Each call returns a fresh token, which satisfies Anthropic's single-use `jti`, and the workload never touches the device key. Covers the endpoint's shape, how the workload is pointed at it (for example an SDK token-provider callback), and local access control on the endpoint.
