---
status: draft
---

# Canopy test site

A test/staging site for Canopy, so Canopy changes are exercised somewhere other than production.

Standing it up (stack, workflow, seeding, which sites report to it, who runs it) is driven by infra and is not specified here.

## Behaviour

- The test site is its own Canopy instance: its own namespace, database, certificate authority and tailnet hostname. Production Canopy is never touched by a test build.
- Every push to main deploys to it. Production deploys are gated separately (N3), so main always reaches the test site first.
- Access matches production: the admin UI is on the tailnet and the device endpoint is public, so a site reports to it the way it reports to production.
- A site is pointed at the test instance by enrolling afresh with it, which issues a certificate from the test instance's CA. A re-enrolled site no longer reports to production.
- It holds a seeded baseline, and live sites report to it. The live sites are a mix, not only the Kubernetes PR and demo deployments, since those do not exercise the production (Linux host) reporting paths.
- Outbound effects are contained, not disabled, so those paths are exercised without touching production:
  - Alerts go to a test Slack channel.
  - DNS publishing targets dedicated test zones, and certificate issuance can use the ACME staging authority.
  - Backups go to dedicated test storage.
  - The instance holds no credentials for production zones, storage or Slack.
- Traffic mirroring from production sites is out of scope for this card.

## Testing notes

- A push to main reaches the test instance and not production.
- A site enrolled with the test instance reports to it, and its certificate chains to the test CA only.
- Test alerts, DNS and backups land in the test channel, zones and storage, with nothing reaching production.
- The Playwright suite can run against the deployed test instance.

## Trade-offs

- A rank of production Canopy was rejected: production would still run the code under test, so an unreleased build would never be exercised.
