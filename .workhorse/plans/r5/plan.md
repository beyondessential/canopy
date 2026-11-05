# OSV vulnerability data and triage

How R5 gets built. Behaviour is in the vulnerabilities (VUL), vulnerability triage (VTR), package reporting (PKG), fleet query interface (MCP) and safety modes (SAFE) specs; the reasoning behind it is in the working doc at `.workhorse/working-docs/r5/working-doc.md`.

## Tech design

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

### Package endpoint

- A dedicated package endpoint keeps package lists out of status history, which records every push in full. It's an addition to the public API, so compatibility holds (see API compatibility, `platform/api-compatibility.md`).


### Model triage details

- Provider is OpenRouter; the first model is GLM Flash. Both are configuration, not code.
- Before deciding on its own, the worker checks the item has never had a person's decision (the decision history has no operator or MCP row), per VTR.
- An item becomes eligible for the model when it's new or reopens: clear its suggestion on reopen so the claim query (no decision, no suggestion) picks it up again.

### Severity

- From the advisory's CVSS vector where it has one (`severity[]`, scored to a qualitative band), else `database_specific.severity` (GHSA, Ubuntu priority). Unrated ranks below low. "Severity rose" compares in this order.

### Ecosystem placement

- `ubuntu` + `VERSION_ID` maps to `Ubuntu:<YY.MM>`, with `:LTS` for LTS releases (even year, `.04`). `debian` maps to `Debian:<major>`. Anything else is kept unplaced.

## Trade-offs

- **OSV scan in `monitor`, AI in a binary of its own.** The scan is ordinary monitoring work. The AI binary isolates the key and any OpenRouter stall, and is where future AI work goes. It's a new single-replica deployment in infra.
- **Evaluating ranges over reopening on any `affected` change.** Reopening on any change would also fire when OSV only adds a fixed version, and would undo triage for edits that change nothing about exposure.
- **Lockfile now, SBOM later.** An SBOM that Tamanu's release CI publishes as a version artifact would also cover Node and the image's OS packages. But it needs a CI change in Tamanu and only describes versions released after that change. The lockfile works for every current version straight away. SBOM ingestion is a follow-up card that adds to the lockfile source, not a prerequisite.


## Build steps

- [x] Migrations and models: advisories, version packages, machine packages (with report digest and ecosystem), matches, decisions, suggestions; `mcp_tokens` grants
- [x] OSV range evaluation (SEMVER via `node-semver`, ECOSYSTEM for Debian/Ubuntu via dpkg ordering, explicit `versions` lists), with unit tests against real OSV records
- [x] Severity derivation from OSV records
- [x] Lockfile reader: fetch at `v{x.y.z}`, parse lockfile v3, drop `dev`, compute workspaces reaching each package and directness
- [x] Package reporting endpoint on the public server, audited as a routine action; `just gen-openapi && just gen-api`
- [x] Scan loop in `monitor`: choose versions and machines that count, querybatch with pagination, fetch revised advisories, write matches, run reopen checks (review dates, material changes)
- [x] Audit actions and an advisory `RecordKind`: decision recorded, item reopened, packages reported
- [x] AI worker binary in `crates/jobs`: config from env, claim loop, OpenRouter structured output (with prompt-JSON fallback), ground enforcement, suggestion or decision writes
- [x] Private API `fns/vulnerabilities.rs`: list (filters), get, record decision (admin, write grade, audited); `just gen-openapi`
- [x] SPA: sidebar section, list page, detail page with decision panel and history, machine page package report line; Playwright coverage with seed helpers
- [x] MCP: caller identity into request extensions from both gates; admin check on tailnet path for the write; token grants at mint and on the token list; `find_vulnerabilities`, `get_vulnerability`, `record_vulnerability_decision` (in `AUDITED_TOOLS`); update the crate's "read-only" doc comment
- [x] Infra: the `ai` binary ships in the image (`cd.yml`). Its deployment, OpenRouter key and model env are configured in infra, outside this repo
