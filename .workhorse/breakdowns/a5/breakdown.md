# Explore canopy as OIDC IdP for workload identity

Work split out of the exploration so the core IdP can be shaped first. The Canopy side (discovery, JWKS, token endpoint, grants, signing key) and the bestool delivery path stay on this card.

## Kubernetes workloads use the cluster's own OIDC issuer

Tamanu pods on Kubernetes have no Canopy identity, only the cluster relay does. Pods authenticate to Anthropic with the cluster's own OIDC issuer registered directly as a federation issuer, so policy lives in Anthropic rules and no Canopy issuance is involved. Covers registering the issuer and rules per cluster and namespace, and documenting the setup for Tamanu's `ANTHROPIC_API_KEY` replacement.

## Kubernetes workloads authenticate through Canopy via the relay

The relay performs a TokenReview on a pod's projected service-account token, and Canopy issues its own JWT carrying the namespace's group and rank. This gives one issuer and one policy and audit surface across machines and pods, and keeps the rule that Canopy holds no credential to the cluster. Depends on the Canopy IdP built on this card.
