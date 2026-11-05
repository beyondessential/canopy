---
status: draft
---

# OSV vulnerability data and triage

Canopy consumes OSV vulnerability data for Tamanu's dependencies and the fleet's Linux packages, and gives operators a page to see and triage what's open.

## Behaviour

### Tamanu dependencies

- A Tamanu version's dependencies are read from the lockfile at that version's release tag in the Tamanu repo.
- Exposure is per version: every application on a version shares that version's dependencies, so a vulnerability in one is open for every application running it.
- This works for every version already released, with nothing needed from Tamanu's release process.

## Implementation options

- Fetching from GitHub has precedent: the Ansible catalogue job (`crates/jobs/src/ansible/catalogue.rs`) reads a file from a repo at a commit.

## Trade-offs

- **Lockfile now, SBOM later.** An SBOM that Tamanu's release CI publishes as a version artifact would also cover Node and the image's OS packages. But it needs a CI change in Tamanu and only describes versions released after that change. The lockfile works for every current version straight away. SBOM ingestion is a follow-up card that adds to the lockfile source, not a prerequisite.

## Open questions

- [ ] Who triages, and what outcomes can a triage decision record?
- [ ] Does a triage decision on a Tamanu version cover every application on it?
