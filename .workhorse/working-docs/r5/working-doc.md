---
status: complete
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

- Vulnerabilities live on their own page, a top-level section of the sidebar between Upgrades and Maintenance. They file no checks and raise no alerts.
- The list is one row per vulnerability: severity, summary and identifier, package and source (Tamanu or Linux), how many applications or machines are exposed, and status. An open item carries the model's suggestion under its status.
- It defaults to open items, sorted by severity, with filters for source and status and a switch to show everything.
- A vulnerability's page shows the advisory, the exposed Tamanu versions (each with its copy of the package and which workspace pulls it in) or exposed machines, the history of decisions with who made each and how, and the decision panel pre-filled from the model's suggestion.
- Mockups: list (`.workhorse/design/mockups/r5/vulnerabilities-list.html`), detail (`.workhorse/design/mockups/r5/vulnerability-detail.html`).

## Implementation options

### Data model

All in `crates/database`, new migrations via `just migration`.

- **Advisories** (`vulnerabilities`): one row per OSV id, caching the record (aliases, summary, details, severity, the `affected` array as JSON) and its `modified` time. Refetched only when OSV reports a newer `modified`.
- **Tamanu packages** (`version_packages`): per Tamanu version, each non-dev package from the lockfile with its version, the workspaces that reach it, and whether it's a direct dependency of any of them. Written once per version; a release tag doesn't move.
- **Machine packages** (`machine_packages`): per machine, its OSV ecosystem (e.g. `Ubuntu:24.04:LTS`) and each source package with its version. Replaced whole on each report, with a digest of the report kept so an unchanged one is a no-op.
- **Matches**: which advisory hits which (package, version) pair, from the last OSV query. Exposure (which production applications and machines) is a join from matches to packages to the fleet at read time, so archiving a box or re-ranking it changes exposure without a rescan.
- **Decisions** (`vulnerability_decisions`): append-only, keyed by (advisory, source), where source is Tamanu or Linux. The newest row is the current decision. Each carries the outcome, reason, review date, issue link, who made it and how (UI, model, MCP), and the advisory's severity and `affected` array at the time, for reopen checks.
- **Suggestions**: the model's latest suggestion per (advisory, source), kept apart from decisions since a suggestion isn't one.

An item's status (open, decided, or reopened) is derived, not stored: open when there's no decision, or the accepted-risk review date has passed, or the advisory has materially changed since the decision.

### Sources

- **Tamanu lockfile.** `beyondessential/tamanu` is public, tags releases `v{major}.{minor}.{patch}`, and has a root `package-lock.json`. Read from raw.githubusercontent at the tag, with no credentials, as the Ansible catalogue reads infra. Lockfile v3's `packages` map carries the workspace layout and `dev` flags. Walking the graph from each workspace gives the dependency path the model needs, even though exposure counts the whole lockfile.
- **Machine packages.** New device endpoint on the public server, `ServerDevice`-authenticated, resolving its machine via `Machine::get_by_device_id`. Shaped like `/backup-capabilities`. OSV's Debian and Ubuntu ecosystems name **source** packages, so bestool reports `${source:Package}` and `${source:Version}` from `dpkg-query`, not binary package names. The OSV ecosystem string comes from `/etc/os-release` (`Ubuntu:<YY.MM>`, with `:LTS` when the release is one). Needs `just gen-openapi && just gen-api`.

### OSV querying

- Match through OSV's `querybatch` (package, version and ecosystem in; ids and `modified` out, results in input order, paginated past 1,000 per query or 3,000 per batch). Then `GET /v1/vulns/{id}` for any advisory that's new or has a newer `modified`. No local copy of the OSV database.
- Query the distinct (ecosystem, package, version) set across the versions and machines that count, not per application or machine. The whole fleet's production boxes are mostly the same few package sets.
- A loop on a fixed tick re-queries everything (advisories appear and change after a scan), and runs early for a new Tamanu version or a changed machine report.
- The scan loop, including the lockfile fetch, runs in the `monitor` binary. It needs no AI access.

### Reopen on material change

- A decision keeps the advisory's severity and `affected` ranges as they stood when it was made. After a scan, Canopy re-evaluates each decided item: it reopens if severity rose, or if a package version exposed now falls outside the recorded ranges. A new Tamanu version that carries a package version already inside the recorded ranges doesn't reopen, which is what lets one decision cover future versions.
- Range evaluation follows OSV's event semantics (`introduced`, `fixed`, `last_affected`, `limit`) plus explicit `versions` lists, per range type.
- npm uses `SEMVER` ranges, evaluated with `node-semver`, which the workspace already depends on for version handling.
- Ubuntu uses `ECOSYSTEM` ranges in Debian version ordering, which needs a dpkg version comparison. Find a maintained crate with `cargo add`, or implement the dpkg algorithm, which is short and well specified.

### AI triage

- AI work runs in its own binary in `crates/jobs`, the only process holding the OpenRouter key, so AI access stays on a small surface. More AI tasks will join it later, so name it for AI work generally, not for triage.
- It's fed through the database: it claims items with no decision and no suggestion (`FOR UPDATE SKIP LOCKED`, as `slack_outbox` claims), so it needs nothing from `monitor` beyond the rows the scan writes.
- For each item it claims, it asks OpenRouter's chat completions API for structured output: outcome, which listed ground applies (if any), and reason.
- Canopy, not the model, decides whether the answer becomes a decision. It does only when the outcome is *not affected* and the model named one of the three listed grounds. Anything else is stored as a suggestion.
- Configured by environment variables, as the Tailscale and Slack credentials are (`TailnetDirectoryConfig::from_env`, `SLACK_WEBHOOK_*_URL`): an API key and a model name. With no key, the step is skipped.
- Check whether the chosen model supports OpenRouter's `response_format` JSON schema. If it doesn't, ask for JSON in the prompt and validate the reply against the same schema, treating an invalid reply as no suggestion.

### MCP

- `crates/canopy-mcp` gains vulnerability tools, two reads and one write, registered as a `vulnerabilities_router`. The write goes in `AUDITED_TOOLS`, which today is empty.
- Tools don't know their caller today: the gates (`require_bearer_token`, `require_tailnet_user`) only log identity. Each gate inserts a caller identity into the request extensions, and rmcp hands tools the HTTP `Parts` (`ctx.extensions.get::<http::request::Parts>()`), so the write tool reads it there.
- The tailnet gate resolves admin status with `TailscaleUser::is_admin` for the write tool only; reads stay open to any tailnet user.
- `mcp_tokens` gains its grants, set at mint in `fns/mcp_tokens.rs` and shown on the MCP access settings page.

### Audit

- New actions: recording a decision (operator, model, or MCP caller as the actor), reopening (Canopy as actor, when a review date passes or an advisory changes materially), and a machine's package report (`tier = Routine`). A suggestion is not audited.
- There's no `RecordKind` for an advisory, so one is added.

### Misc

- A dedicated package endpoint keeps package lists out of status history, which records every push in full. It's an addition to the public API, so compatibility holds (see API compatibility, `platform/api-compatibility.md`).

## Trade-offs

- **OSV scan in `monitor`, AI in a binary of its own.** The scan is ordinary monitoring work. The AI binary isolates the key and any OpenRouter stall, and is where future AI work goes. It's a new single-replica deployment in infra.
- **Evaluating ranges over reopening on any `affected` change.** Reopening on any change would also fire when OSV only adds a fixed version, and would undo triage for edits that change nothing about exposure.
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
- [x] Follow-up cards: SBOM ingestion, *will fix* to Workhorse card, and bestool's package reporting (in bestool's repo). In the card breakdown.
