# Automate Deployments

## Overview

*Automate Everything* put the fleet's deployment work on record in Canopy: upgrade plans, a desired version per environment, leases that refuse a colliding run, migration testing against real data, and deployment artefacts.
*Seedling in production* gave Seedling the surface to serve Tamanu the way the fleet serves it today.
Neither has yet changed how a deployment is actually carried out: no host runs on Seedling, an upgrade plan is still a note a person executes by hand, and a reporting schema still reaches production as SQL somebody runs.

This project closes that gap.
Seedling goes into production across the Linux fleet; on hosts that run it, Canopy's upgrade plans stop being a record and start performing the upgrade; reporting schemas and reports are generated and installed without a manual step; and both products get the QA they need to be trusted with all of that.

**This is a multi-repo project.** Canopy is where it is planned and tracked, but the code lands in Canopy, Seedling, bestool, Tamanu, and ops. Cards are shaped here and move to their respective workspace when work starts on them.

## Sequencing

- **Now, unblocked**: the Seedling carry-over from phase 1; production Tamanu definitions; the Canopy test site; the reporting-schema polish pass; the three reporting scoping decisions
- **Once the carry-over lands**: the Ansible stages, rewritten against what shipped, then the pilot. The pilot becomes the first Seedling test site
- **Once a Seedling test site exists**: Canopy-driven upgrades, built and exercised there rather than on production; the Seedling UI pass
- **Once the pilot proves out**: fleet rollout in risk order, and greenfield Linux deployments on Seedling by default
- **Alongside throughout**: the QA pass and its automation, and reporting, which depends on Seedling only for its Seedling consumer

## 1. Seedling in production

### Carried over from phase 1

Specified in *Seedling in production* and not built.

- **Integrate the new Canopy TLS issuance.** Warm-cert observation reads only Caddy's on-disk cache, so a certificate provisioned off the `:80` path does not satisfy `rt.warm_certs().ready()`. Canopy now issues TLS certificates and bestool already has an implementation, so Seedling follows along: this is the certificate path for hosts where another process holds `:80`, and it covers warm-cert observation of certificates Seedling did not place in the cache

Canopy-driven backups and removal of Seedling's own backup framework, also phase 1, do not gate cutover: the fleet's app-data backups stay host-side while PostgreSQL does.

### Production definitions

The definitions in Seedling's `apps/` are demos and stay that way.
The production Tamanu definitions live in the Tamanu repo, change in the same commit as the behaviour they describe, and are published as OCI artefacts alongside Tamanu's images, using the provenance and fetch mechanism phase 1 built.

What they must say is set by Tamanu's move off json5 config: everything that described the deployment to Tamanu moves into Tamanu's internal settings, in the database, and crosses a cutover untouched.
A small amount of per-host state and a few structural requirements remain for the definition to carry — among them the per-server config key and the range of Tamanu versions the definition supports — and are worked out when the definition is carded.

The mSupply definition follows the same regime as a stretch goal, promoted from its draft once the Tamanu definition has proven out.

### The transition

Four playbooks, each independently runnable and idempotent, as `docs/plans/adhoc-to-seedling-migration.md` in the ops repo lays out: install, adopt, cut over, decommission.
Install is inert by construction, since Seedling with no apps registered performs an idle teardown, and can go fleet-wide as soon as its package preconditions are confirmed.
Cutover is the only stage with a blast radius.

Rewrite adopt and cutover against what shipped rather than adapting the existing step lists.
Under the staged ingress takeover an app is fully installed, routed and TLS-provisioned with no host DNAT rules, then one explicit operator action takes traffic.
This moves most of cutover into verification done ahead of time, and definitions now arrive as OCI references rather than pushed text.

Properties that must survive the rewrite:

- The Tamanu version is held constant across a cutover, so nothing runs a schema migration and re-enabling the old units is a real rollback
- The old workers stop before Seedling's start, so the two stacks never write to the shared database concurrently. This is what makes rollback lossless, and it is not obvious from the step list
- Verification reads something encrypted under the config key: a completed sync round on a facility, the reporting secret path on a central. An app reporting ready does not prove the key is right
- Rollback is one step at any point before decommission
- Decommission is a separate run on operator judgement, and does not remove the old config-key secret
- Once a host is Seedling-managed, the legacy install and upgrade playbooks refuse to run on it without an explicit override

Ops prerequisites from the same plan: bestool suppresses Tamanu service checks while keeping cluster and DB checks (C3); Seedling's package dependencies resolve on the fleet's OS and architectures (C4); an Ansible-owned ctl client key (C5).

New Linux deployments run install and adopt, then a plain install instead of a cutover, and become the default as soon as the pilot proves out, so the legacy layout stops growing.

### Pilot, then rollout

The pilot is a dedicated AWS environment built through the existing ad-hoc path and then migrated, so it exercises the whole transition rather than a greenfield install.
It stays up afterwards as a Seedling test site (section 3).

Rollout follows recoverability, not prerequisite count.

1. Pilot environments
2. AWS-hosted production, which tolerates tier 2 gaps behind the stack-wide security group and AWS Backup
3. On-prem production, which needs tier 1 and tier 2 in full
4. The `.local` class last: no certificate needed, but the hardest networks and the hardest rollbacks

PostgreSQL stays a host package throughout.
Moving it is a later project, for the reversibility reasons the phase 1 PRD and the ops plan set out.

## 2. Upgrades under Canopy's control

On a Seedling host an upgrade is one param set: changing Tamanu's `version` runs the definition's upgrade closure.
So on an environment with Seedling enabled, an upgrade plan becomes an active control: when the plan is ready and its window opens, Canopy directs Seedling to perform the upgrade, and Seedling reports what it did.
Environments not on Seedling keep today's passive plans, unchanged.

This is *Control at a distance* from the phase 1 Seedling PRD, which was specified there and not built.

### The channel

Seedling already reports to Canopy every sixty seconds through a connected client, and the report response is specified to carry instructions back (`r[canopy.report.backup-prompt]`), which Seedling currently receives empty and ignores.
That response is the inbound path.
It is poll-driven, bounded to the report cadence, and needs no new listener and no inbound authority through the relay, which is deliberately outbound-only.
Keep it that way.

An instruction carries an identifier, and Seedling's next reports say what became of it: accepted, refused and why, in progress, done, failed.
An operator watching the host sees what Canopy asked for and what it caused.

### What an upgrade instruction carries

An upgrade is not always a param set.
Across a regime change the definition and the version move together in one atomic update, so the upgrade runs under the definition that knows the new regime.
An upgrade instruction carries a target version and a definition reference, and Seedling refuses a combination the definition does not support.

Seedling may refuse for other reasons too: the definition's validators reject the change, an operation is already in progress, or the host is configured to accept no remote direction.
A refusal is reported, not blindly retried.

The settable surface is narrow on purpose: an upgrade instruction, not arbitrary remote param writes.

### Readiness

A plan becomes executable only when what already exists around it agrees.

- Migration testing has a passing verdict for this environment at the target version, recent enough to trust
- The upgrade manifest decisions for the versions in the gap are filled in (*Version upgrade schema manifest system*, J3), and are passed to the upgrade once that pathway exists
- The environment's lease is free. The upgrade takes it for its duration, so an Ansible run cannot collide
- A maintenance window is declared over the upgrade, so incidents are suspended rather than raised

### Outcome

Canopy records the outcome on the plan: started, finished, duration, and on failure where and with what.
A plan is met by the environment reporting the target version, as today.
A failed upgrade leaves the plan open and the environment in maintenance until an operator lifts it.

Rollback is the definition's business, not Canopy's: a Tamanu upgrade that has run migrations is not reversed by setting the old version, and Canopy does not offer a rollback it cannot honour.

## 3. QA

Canopy and Seedling are tested per card, by whoever builds the card.
Neither has had a pass by someone whose job is to find what the builder did not think of, and neither has an environment where it runs against data that looks like the fleet without being the fleet.
Both are internal products and do not need Tamanu's level of QA; they need enough that we trust them, and enough automation that the trust survives the next change.

This strand is a project in its own right for a tester rather than per-card testing, and is where Sima can contribute most.
It also takes over the ongoing-QA question *Automate Everything* raised and never carded.

### A Canopy test site

A standing Canopy deployment, separate from production, with a synthetic fleet: groups across every rank, machines and applications of each type, versions spread across releases, backups that succeed and fail, incidents that open, flap, escalate and close, upgrade plans that are open, late, met and withdrawn.
The synthetic data is generated and reproducible, so the site resets to a known state.

On top of that, some PR and demo Tamanu deployments report into it, so it sees live health checks, backups and restores from software that is actually running, without any of it mattering.
The Seedling test sites report into it as well, which is where section 2 is exercised.

It is also the natural staging environment for Canopy itself, which *Gate canopy deploys on something other than every push* (N3) lists as one of its options.

### Seedling test sites

The pilot from section 1, kept after migration, and a greenfield one.
Between them they cover adoption, a plain install, upgrades, and the Canopy-driven paths.
They are rebuilt through the same playbooks the fleet uses, so rehearsing a Seedling release on them also rehearses the playbooks.

### The UI pass

A structured pass over each product's UI against the test sites: every page, the flows an operator actually performs, empty, large and broken data, and what an operator is left believing after each action.
Findings are carded in the product's workspace.

Canopy's UI is due a redesign (*Redesign UI with left sidebar navigation*, W3), and the two are sequenced deliberately.
A pass on today's UI finds the problems the redesign should fix and gives it a baseline, while automation written against today's layout is mostly rewritten after it.
So: pass first, findings feed the redesign, and automation targets flows rather than layouts so the tests outlive it.

### Automation

Both repos already run Playwright, Canopy's sharded across CI.
The pass's flows become Playwright tests against a seeded fixture, so they run on every PR and not only on the test site.
Expect the tooling to need adapting: fixtures seeded from the same generator as the test site, and for Seedling, driving a real host rather than only the web frontend.

### Ongoing QA

Settle what QA looks like after this project, for Canopy, Seedling, and the deployment and Ansible work around them: what gets a tester's pass and when, what the test sites are for between releases, and who keeps them running.

## 4. Reporting

*Automate Everything* brought reporting-schema generation into Canopy's view through discovery and deployment artefacts.
What it did not reach is reports themselves, and getting schemas and reports onto a deployment with no manual step.

### Reporting-schema generation

Take the pipeline as it stands after *Automate Everything* and make it something VitiOps runs without help: commissioned from Canopy, produced against a managed restore replica, versioned and attributed as a deployment artefact, with failures that say what failed.
The first card is a pass over the current pipeline listing what is rough.

### Standard and deployment reports

- **Standard reports** are the same for every deployment, published fleet-wide
- **Deployment reports** are generated the way reporting schemas are: from a common definition, customised for one deployment based on its data

A deployment report is a deployment artefact, with the same privacy property: deployment-specific artefacts are not enumerable by anyone who should not see the client list.
It is generated when the deployment is configured for it, against a particular schema version, and records which.

### Applying schemas and reports

The consumer lands in both bestool and Seedling, because the transition to Seedling is long and bestool serves every deployment that has not moved.
Same behaviour, two implementations, one card per repo.

- Pull the deployment's current reporting schema and apply it
- After the schema is installed or updated, pull and install the standard and deployment reports that go with it
- Report what was applied back to Canopy, so a deployment's reporting state is known rather than assumed

Reports never install ahead of the schema they need.

### Needing a scoping decision from Edwin

Included as raised.
Each needs a decision on whether it belongs here, in a Tamanu project, or nowhere, before it is carded.

- **Survey reporting schema generation in Tamanu**
- **Mark and gate standard and deployment reports in Tamanu**
- **Track report requirement vs reporting schema version**

## Success criteria

- A production Tamanu definition is published by Tamanu's release process and fetched by Seedling by reference
- The pilot is migrated through the four stages, a post-flip rollback is rehearsed on it with no lost writes, and a first production deployment runs on Seedling
- New Linux deployments are built on Seedling by default
- An upgrade plan on a Seedling test site is executed by Canopy at its window with no one touching the host; a refused upgrade reports its reason; an Ansible run mid-upgrade is refused by the lease
- The Canopy test site resets to a known synthetic state and has live non-critical sources reporting into it
- Each product has had a full UI pass with findings carded, and the flows it covered run in CI
- A reporting schema and its deployment report are generated from Canopy and installed in order by bestool and by Seedling, and Canopy shows each deployment's applied versions

## Open questions

- Which certificate path is the default for public hosts
- Where the mSupply definition lives
- Who confirms a ready upgrade plan, and whether production needs the authorisation unlock *Automate Everything* proposed
- Whether facilities upgrade with their central, after it, or on their own plans
- How long a migration-testing verdict stays valid, and what happens when a window opens during a Seedling operation already in progress
- Which PR and demo deployments feed the Canopy test site, and whether it is a separate deployment or a separate rank of production Canopy
- What inputs customise a deployment report, who authors the common definition, and what happens to a deployment report when its schema moves on
- The three reporting scoping decisions above