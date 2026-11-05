---
base: default
---

# OSV triage follow-ups

Work that builds on vulnerability triage but is left out of this card.

## Read Tamanu dependencies from a published SBOM

Tamanu's release CI publishes an SBOM for each release as a version artifact, and Canopy reads a version's dependencies from it where one exists, falling back to the lockfile at the release tag where none does. An SBOM can also cover Node and the container image's OS packages, which the lockfile can't, so applications on a cluster gain the OS packages they lack today. This needs the Tamanu release CI change as well as Canopy's side.

## Start a Workhorse card from will-fix items

Gather the vulnerabilities triaged as will fix and start a Workhorse card for the fix, carrying the advisories, affected packages and exposed versions. Canopy has no Workhorse integration yet, so this settles how it talks to Workhorse and who the card is created as.

## Report installed packages from bestool

bestool reports its box's installed source packages to Canopy's package reporting endpoint, with the distribution and release from `/etc/os-release`, sending the list when it changes. This work is in bestool's repository; Canopy's endpoint is specified in the package reporting spec.
