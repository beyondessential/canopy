---
status: draft
---

# Canopy test site

A test/staging site for Canopy, so Canopy changes are exercised somewhere other than production.

## Behaviour

- The test site is its own Canopy instance: its own namespace, database, certificate authority and tailnet hostname. Production Canopy is never touched by a test build.
- It is fed two ways, and both are in scope:
  - A seeded baseline, so Playwright and manual QA see deterministic fleet state.
  - Live reporting from a mix of test sites. These are not only the Kubernetes-based PR and demo deployments, since those do not exercise the production (Linux host) reporting paths.
- Every push to main deploys to the test site automatically. Production deploys are gated separately (N3), so main always reaches the test site first.
- Access matches production: the admin UI is on the tailnet and the device endpoint is public, so a test site reports to it the way it reports to production.
- A site is pointed at the test instance by enrolling afresh with it, which issues a certificate from the test instance's CA. A re-enrolled site no longer reports to production.
- Outbound effects are contained, not disabled, so the paths are exercised without touching production:
  - Alerts go to a test Slack channel.
  - DNS publishing targets dedicated test DNS zones, and certificate issuance can run against the ACME staging authority.
  - Backups go to dedicated test backup storage.
  - The test instance holds no credentials for production zones, storage or Slack.
- The seeded baseline is loaded by reusing the Playwright suite's seed helpers (`private-web/e2e/seed.ts`), so the fixtures QA sees are the ones the e2e tests rely on. It runs on deploy or on demand.
- Traffic mirroring from production sites is out of scope for this card.

## Implementation options

- **Second Pulumi stack of the existing `canopy` project in ops.** The project derives the namespace (`tamanu-meta-<stack>`), default domain, and CA from the stack name, and already has a `stage` config used for billing labels. A second stack is the cheapest way to a separate instance.
  - The CD workflow (`cd.yml`) deploys through one `release` environment with one `PULUMI_STACK` and `OPS_REF`, so a second target needs its own environment or a matrix.

## Open questions

- [ ] Image tag the test site deploys: `sha-<short>` is fine while only the test site consumes every push, but it must stay consistent with whatever N3 settles on for production.
- [ ] Which test sites report to it (a mix of Linux hosts and Kubernetes PR/demo deployments; the exact list is not settled).
- [ ] Who keeps it running, and what it holds between QA passes (the PRD's "Ongoing QA" section).
- [ ] The PRD's alternatives (dedicated QA sites; a clone site holding a production snapshot) were set aside in favour of a separate instance. Does a production snapshot still get loaded into it later?

- [ ] The seed helpers are TypeScript and run against a database from the Playwright fixture. Reusing them on a deployed instance needs a way to run them against its database, or a Rust-side equivalent.
- [ ] Is the seed re-applied on every deploy (resetting state) or only on demand (preserving what enrolled sites have reported)?

## Testing notes

- A push to main reaches the test instance and not production.
- A site enrolled with the test instance reports to it, and its certificate chains to the test CA only.
- Test alerts, DNS and backups land in the test channel, zones and storage, with nothing reaching production.
- The Playwright suite can run against the deployed test instance.

## Trade-offs

- A rank of production Canopy was rejected: production would still run the code under test, so an unreleased build would never be exercised.
