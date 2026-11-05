---
status: draft
---

# OSV vulnerability data and triage

Canopy consumes OSV vulnerability data for Tamanu's dependencies and the fleet's Linux packages, and gives operators a page to see and triage what's open.

## Behaviour

### What counts as exposure

- Only production rank counts. A Tamanu version is scanned when a live production application runs it, and a box's packages are scanned when it's a live production machine. Clone, demo, test and dev don't count, and neither does anything pending or archived.
- The newest published Tamanu release is scanned too, whether or not anything runs it, so an upgrade target can be triaged before production moves to it. Its items show zero applications exposed.
- Applications on a cluster have Tamanu dependencies like any other, but no Linux packages of their own. The image's OS packages are the SBOM card's to cover.

### Tamanu dependencies

- A Tamanu version's dependencies are read from the lockfile at that version's release tag in the Tamanu repo.
- Exposure is per version: every application on a version shares that version's dependencies, so a vulnerability in one is open for every application running it.
- This works for every version already released, with nothing needed from Tamanu's release process.
- Every non-dev dependency in the root lockfile counts for every Tamanu application, whichever package (central, facility, web) it runs. A frontend-only package's vulnerability shows against servers too; triage marks those not affected.

### Linux packages

- bestool reports a box's installed packages to a dedicated device API endpoint, sending the list when it changes rather than on every status push.
- Exposure is per machine: each box is exposed to the vulnerabilities in its own packages.
- The bestool side is a change in bestool's own repo.

### Triage

- Operators triage, gated as admin like any other operator action.
- An item without a decision is *open*. A decision is one of:
  - *not affected*, with a reason;
  - *accepted risk*, with a reason and a date to review it again;
  - *will fix*, optionally linking an issue.
- A decision on a Tamanu dependency vulnerability attaches to the vulnerability for Tamanu as a whole, not one version. It applies to every version carrying an affected release of the package, including versions released later, so a new Tamanu release doesn't reopen what's been triaged.
- A decision on a Linux package vulnerability covers the whole fleet: one decision applies to every box with an affected package, since boxes are built alike.
- An accepted risk whose review date has passed is open again.
- When OSV revises a decided advisory, the item reopens only on a material change: its severity rises, or its affected range now reaches a package version the decision didn't cover. Any other revision updates the advisory in place and the decision stands.
- An item no application in the fleet is exposed to any more drops off the open list without needing a decision.
- The *will fix* items can be listed on their own. Turning them into a Workhorse card for the fix is a later card.

### AI triage

- A model looks at each new item and either marks it *not affected* itself, where the case is obvious (e.g. a browser-only package's vulnerability against the facility server), or suggests an outcome and reason for an operator to take, edit or ignore.
- A decision the model made is shown as the model's, distinct from an operator's, and an operator can replace it like any other decision.
- A suggestion is not a decision: the item stays open until someone acts on it.
- Runs through OpenRouter, on a small fast model (GLM Flash to start).
- The model sees the OSV advisory and where the package sits in Tamanu's dependency tree: which workspace pulls it in, and whether it's direct or transitive. The tree is what lets it tell a browser-only package from a server one, even though exposure counts the whole lockfile.
- The model dismisses an item on its own only on these grounds, fixed in Canopy:
  1. the package is reached only from the web frontend, and the flaw needs server-side execution;
  2. the package only runs at build time, and the flaw needs it to run in production;
  3. the advisory affects only an OS or architecture the fleet doesn't run.
- On any other grounds it suggests.

### Triage over MCP

- Responders can point a more capable model of their own at the triage list for a second pass, through Canopy's MCP interface (see Fleet query interface, `private-server/mcp.md`).
- Queries: list vulnerabilities (with the page's filters), and get one vulnerability (advisory, exposure, dependency path, decision history, the model's suggestion).
- Writes: record a triage decision. This is the first write the MCP interface carries, which today is read-only by spec.
- On the tailnet path, a write needs the caller to be an admin; reading stays open to any tailnet user.
- An access token carries a triage grant, chosen when it's minted and off by default. A token without it stays read-only, which is every token minted before this.
- A decision written over MCP is the caller's (the tailnet user, or the token's name), marked as made over MCP, so it reads apart from both an operator's decision in the UI and the in-Canopy model's.
- An MCP write may replace any decision, an operator's included. The history keeps what it replaced; the responder pointing the agent owns the call.

### Where it shows

### Where it shows

- Vulnerabilities live on their own page, a top-level section of the sidebar between Upgrades and Maintenance. They file no checks and raise no alerts.
- The list is one row per vulnerability: severity, summary and identifier, package and source (Tamanu or Linux), how many applications or machines are exposed, and status. An open item carries the model's suggestion under its status.
- It defaults to open items, sorted by severity, with filters for source and status and a switch to show everything.
- A vulnerability's page shows the advisory, the exposed Tamanu versions (each with its copy of the package and which workspace pulls it in) or exposed machines, the history of decisions with who made each and how, and the decision panel pre-filled from the model's suggestion.
- Mockups: list (`.workhorse/design/mockups/r5/vulnerabilities-list.html`), detail (`.workhorse/design/mockups/r5/vulnerability-detail.html`).

## Implementation options

- A dedicated package endpoint keeps package lists out of status history, which records every push in full. It's an addition to the public API, so compatibility holds (see API compatibility, `platform/api-compatibility.md`).
- OSV's batch query API takes package, version and ecosystem (npm for Tamanu; the box's Ubuntu release for Linux, so bestool's report needs to carry it). A job re-queries known packages on a schedule, since advisories appear and change after a version is scanned.
- Triage decisions are audited actions (`database::audit::Action`), whoever makes them: an operator, the in-Canopy model, or an MCP caller.
- The OpenRouter key needs a home, probably the secret store. With none set, AI triage doesn't run and items just stay open.
- Fetching from GitHub has precedent: the Ansible catalogue job (`crates/jobs/src/ansible/catalogue.rs`) reads a file from a repo at a commit.

## Trade-offs

- **Lockfile now, SBOM later.** An SBOM that Tamanu's release CI publishes as a version artifact would also cover Node and the image's OS packages. But it needs a CI change in Tamanu and only describes versions released after that change. The lockfile works for every current version straight away. SBOM ingestion is a follow-up card that adds to the lockfile source, not a prerequisite.

## Open questions

- [x] Who triages, and what outcomes can a triage decision record? Operators; not affected / accepted risk / will fix.
- [x] Does a triage decision on a Tamanu version cover every application on it? Yes, and every other version carrying the affected package.
- [x] Gathering *will fix* items into a Workhorse card: on this card or a later one? A later card.
- [x] What happens when an accepted risk's review date passes? It's open again.
- [x] How do a box's Linux packages reach Canopy? bestool, on a new endpoint.
- [x] Do vulnerabilities feed checks? No, the page only.
- [x] What does the model get to look at, and what counts as obvious enough to dismiss on its own? Advisory plus dependency path; three listed grounds.
- [x] MCP writes: who may write on each path, and how is a decision made over MCP attributed? Admins on the tailnet path, granted tokens on the token path; attributed to the caller, marked as over MCP.
- [x] May an MCP write replace an operator's decision? Yes, history kept.
- [x] Which Tamanu versions get scanned? Those running at production rank, plus the newest published release.
- [x] What reopens a decided item when OSV revises it? Rising severity, or an affected range that reaches a package version the decision didn't cover.
- [ ] Follow-up cards: SBOM ingestion, *will fix* to Workhorse card, and bestool's package reporting (in bestool's repo).
