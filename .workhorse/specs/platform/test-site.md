---
id: TEST
---

# Canopy test site

The test site is a Canopy instance that exists so Canopy changes are exercised somewhere other than production.
It runs unreleased builds, receives reports from real sites, and is the place QA is done.

## A separate instance

The test site is its own installation of Canopy, with its own database, certificate authority, and hostname.
It is not a rank of production Canopy: production Canopy holds none of its state, and a test build never runs in production.

The test site exposes the same surfaces as production Canopy.
Its private admin interface is reachable on the tailnet and its device interface is reachable from the internet, so a site reports to it the way it reports to production.

## Builds

Every build of main is deployed to the test site.
A build reaches the test site without being promoted to production.

## Sites that report to it

A site is pointed at the test site by enrolling with it afresh, and is issued a certificate by the test site's certificate authority.
That certificate is trusted by the test site and by no other Canopy instance, so a site enrolled with the test site reports to it and not to production.

The sites that report to the test site are a mix of Linux hosts and Kubernetes deployments, so the reporting paths production relies on are exercised there as well as the cluster ones.

The test site also holds baseline data, so QA starts from a known state of the fleet and does not depend on what happens to have reported.

## Containment

The test site's outbound effects are directed at test resources, so each is exercised without reaching production.
Alerts go to a test Slack channel.
Domain publishing targets test zones, and certificate issuance can be run against the authority's staging environment.
Backups go to test storage.
The test site holds no credential for production zones, storage, or Slack.
