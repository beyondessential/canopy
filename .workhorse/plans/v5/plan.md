# Vuln tracking improvements: plan notes

## Data carried over

- `repository_packages` has no `lockfile` column; its key is (repository, ecosystem, name, version). Package-lookups use a repository's whole read.
- Existing `tamanu` items, matches and decisions become items and decisions of the Tamanu repository source; the source column moves from a two-value enum to a source name (a repository name, or `linux`).
- Review dates on existing accepted risks are dropped and those decisions stand.
- Decisions the model made (via `model`) are withdrawn and become suggestions, with a basis of insufficient information, so a person looks at each. The withdrawal is done by the scan job on start (idempotent, skipping already-withdrawn ones) rather than in SQL, so each is audited as Canopy's.
- Existing suggestions are deleted, so the model suggests again under the new rules with a basis.
- Existing reopenings become lapses; a stored `review_due` reason still parses, for history.
- Existing decisions are whole decisions.
- Per-version Tamanu lockfile tables (`version_lockfiles`, `version_packages`) give way to per-repository package tables read at the default branch's head.

## Notes

- The repository list is hardcoded for now; moving it to the database with an edit UI is W5. Scanning container images in clusters is explored in X5.
- Scope vocabulary follows SPDX 3 lifecycle scopes (runtime, build, development, test, design, other), with `optional` as a separate flag. npm supplies runtime/development/optional from the lockfile; Cargo supplies runtime/build/development from manifest dependency kinds. uv supplies runtime, development (dependency groups) and optional (extras) from `uv.lock`.
- Only the shortest chain per workspace package and scope is stored, since npm trees can have very many paths to one package.
- Closed is a flag beside triage status, not a status value; the list's "show closed" switch and MCP's include-closed flag both drive it.
- Repositories are public and read without credentials, as infra's catalogue is.
- Search wants a trigram index (pg_trgm) over identifier, aliases, summary, package, workspace packages and source, with details weighted lower.


## Design

- **Decisions as entries.** A decision is a list of entries, each with workspace packages (or none, for the catch-all), an outcome, a reason and an issue link. A whole decision is a single catch-all entry, so "covers packages that come to reach later" is one rule for both. Status is the shared outcome when every entry agrees, otherwise mixed.
- **Withdrawals, edits and lapses hang off a decision.** Withdrawals (one per decision) and lapses (one per decision, the renamed reopenings) each have their own table. Edits (many per decision, each naming an entry and keeping the earlier text) also have their own table. An item needs triage when it has no decision, or when its newest decision is withdrawn or has lapsed.
- **Matches are per package version, not per source.** Several repositories can carry the same npm version, so `vulnerability_matches` loses `source`. Items are derived by joining matches to `repository_packages` (repository items) or to live production `machine_packages` (Linux items). That keeps "ranking or archiving a machine changes exposure without a rescan" true at read time.
- **Closed is computed when read.** It is computed from that join, not from the stored `counts` flag, which goes away.
- **One SQL item view.** The list, its facet counts, MCP's `group_by`, the sidebar count and the model's claim all read the same CTE. Today's in-memory filtering and sorting goes away.
  - Facet counts each drop their own filter.
  - Paging is offset-based behind an opaque cursor, since sorting works on any column.
  - Minimum fix versions are worked out in Rust for the returned page only.
- **Search.** Use `word_similarity` with pg_trgm, which audit already enables. Index two things with `gin_trgm_ops`:
  - a per-item `search_text` (identifier, aliases, summary, package names, workspace packages, source), kept by the scan;
  - `vulnerabilities.details`.

  Rank is the main-field score plus half the details score. A search orders by rank unless a sort is chosen.
- **Reading repositories without hitting GitHub's rate limit.** Unauthenticated `api.github.com` allows 60 requests an hour.
  - Watch each repository's head through git smart HTTP (`/info/refs?service=git-upload-pack`, which resolves `HEAD`), which uses no API quota.
  - Only when the head moves, list the tree once through the API (`git/trees/{sha}?recursive=1`).
  - Fetch lockfiles and manifests from `raw.githubusercontent.com` at that sha.
- **Lockfile discovery.** Every `package-lock.json`, `Cargo.lock` and `uv.lock` in the tree counts, `node_modules/` excluded. A workspace package's name is qualified with its lockfile's directory only where two lockfiles in one repository share a member name.
- **Cargo scopes** come from each member's `Cargo.toml` tables, including target-specific ones: `[dependencies]` is runtime, `[build-dependencies]` is build, `[dev-dependencies]` is development. Transitive packages walked through `Cargo.lock` inherit the scope of the edge that leads to them.
- **npm scopes** come from each workspace entry's `dependencies` / `devDependencies` / `optionalDependencies` in the lockfile's `packages` map. The existing walker in `lockfile.rs` gains an edge kind instead of dropping `dev` entries.
- **uv:**
  - Workspace members are the packages with an editable, virtual or workspace source.
  - `dependencies` is runtime, `optional-dependencies` is runtime and optional, and `dev-dependencies` is development.
  - Names are normalised to PEP 503 for OSV, ecosystem `PyPI`.
  - OSV ranges for PyPI are `ECOSYSTEM`-typed, so `osv.rs` needs PEP 440 ordering (`pep440_rs`, added with `cargo add`).
- **Privacy fix in the prompt.** Today's Linux prompt sends up to five machine names. Under the triage spec (VTR), a Linux item goes to the model as package and advisory only.

## Checklist

### Schema
- [x] `just migration vulnerability_repositories`. In it:
  - add `repository_reads` (repository, commit, read_at, last_attempt_at, failure), `repository_packages` (repository, lockfile, ecosystem, name, version) and `repository_package_paths` (repository, ecosystem, name, version, workspace_package, scope, optional, direct, chain);
  - drop `version_lockfiles` and `version_packages`.
- [x] In the same migration, rework the items:
  - drop the `source` check on items, decisions, lapses and suggestions (the source becomes a repository name or `linux`);
  - drop `vulnerability_items.counts` and add `search_text` with a trigram index, plus a trigram index on `vulnerabilities.details`;
  - drop `source` from `vulnerability_matches` and re-key the table on (vulnerability_id, ecosystem, package, package_version).
- [x] In the same migration, rework the decisions:
  - drop `review_by` and its check, and `ground`;
  - add `vulnerability_decision_entries` (decision_id, position, workspace_packages TEXT[] NULL for catch-all, outcome, reason, issue_url, plus the reason-needed check), filled from existing decisions as one catch-all entry each, then drop outcome/reason/issue_url from `vulnerability_decisions`;
  - add `covered_held TEXT[]` (the workspace packages the decision covered when made), empty for existing ones;
  - add `vulnerability_decision_withdrawals` (decision_id unique, reason, actor, via, created_at) and `vulnerability_decision_edits` (decision_id, position, reason, issue_url, earlier reason, earlier issue link, actor, via, created_at);
  - rename `vulnerability_reopenings` to `vulnerability_lapses`.
- [x] In the same migration, rework the suggestions: delete existing ones, replace `outcome`/`ground`/`reason` with `entries JSONB` (same shape as decision entries) plus `basis TEXT NOT NULL` (checked against the four bases).
- [x] Write `down.sql`, run `just migrate`, regenerate `schema.rs`.

### Database (`crates/database`)
- [x] `vulnerabilities.rs`: `Source` becomes `Repository(name)` / `Linux`, serialised as the name. Add a `REPOSITORIES` const (name, GitHub slug) for the ten repositories, and check each slug exists before relying on it. Map ecosystems `npm`, `crates.io` and `PyPI` to repository items; everything else is Linux.
- [x] `packages.rs`:
  - replace `VersionPackage` with `RepositoryPackage { lockfile, ecosystem, name, version, paths: Vec<Path { workspace_package, scope, optional, direct, chain }> }`;
  - add `Scope` (the SPDX six);
  - add `RepositoryRead::store(conn, repo, commit, packages)`, which replaces a repository's rows in one transaction;
  - add `RepositoryRead::note_failure` and `RepositoryRead::stale(conn, after)` for the self-alert.
- [x] Replace `CountedFleet` / `Exposure` / `refresh_counts` with:
  - `Counted::all(conn)`: distinct (ecosystem, name, version) across `repository_packages` and live production machines;
  - an `item_view` SQL fragment giving per item its source, status, closed, scopes, workspace packages, affected packages, machine count, suggestion presence and search rank.
- [x] `replace_matches` / `prune_matches` without `source`. Items are inserted for every (advisory, source) the join finds, and their `search_text` is refreshed.
- [x] `osv.rs`:
  - add `VersionKind::Pep440` for `PyPI`;
  - add `min_fix_version(affected, ecosystem, name, version)` (lowest fix above the version);
  - rename `Reopening` to `Lapse`, keep the `review_due` variant for parsing old rows, and add `NewWorkspacePackage { name }`.
- [x] Decisions:
  - `NewDecision { entries: Vec<Entry> }` with `validate(workspace_packages_now)`. It refuses: an uncovered package with no catch-all, a package in two entries, a package that doesn't reach the advisory, more than one catch-all, a catch-all not last, and a split on a Linux item. It applies the outcome field rules per entry.
  - `Decision::record` keeps `covered_held`.
  - `Decision::withdraw(conn, item, reason, actor, via)`, which refuses when there is no current decision or it is already withdrawn.
  - `Decision::edit(conn, item, entry selector (package or catch-all), reason?, issue_url?, actor, via)`.
- [x] `Lapse::pass(conn, counted)`:
  - drop the review-date branch;
  - add a workspace-package branch that lapses a split without a catch-all when a package now reaches the advisory outside `covered_held`;
  - on a lapse, delete the suggestion and clear `model_attempted_at`.
- [x] `Suggestion { entries, basis, model, created_at }` with `Basis` (dependency_path_only, advisory_text_only, dependency_path_and_advisory_text, insufficient_information). `Suggestion::record` checks the entries against the item's workspace packages with the same validation as a decision.
- [x] Listing:
  - `ListQuery { source, status, scope, severity, has_suggestion, include_closed, affected_package, workspace_package, search, sort (column, direction), cursor, page_size }`;
  - `ItemSummary::list` returns `{ items, total, next_cursor }`;
  - `ItemSummary::facets` returns counts per choice of source, status, scope, severity and suggestion, each without its own filter;
  - `ItemSummary::group(query, by)` groups by affected package, workspace package, scope, source or status;
  - `ItemSummary::needs_triage_count`.
- [x] `ItemDetail`:
  - repository items list affected packages, each with version, minimum fix version and paths (workspace package, scope, optional, direct, chain), and the commit read at;
  - Linux items list machines with installed version and minimum fix version;
  - the history merges decisions (with entries and an "edited" marker), edits, withdrawals, suggestions and lapses, newest first;
  - expose `withdrawn` / `edited` on the current decision.
- [x] `claim_for_model`: items that need triage, aren't closed and have no suggestion, read through the item view.
- [x] `audit.rs`:
  - `VulnerabilityDecided` detail gets `entries` and drops `review_by`;
  - add `VulnerabilityDecisionWithdrawn` (`vulnerability_decision_withdrawn`) and `VulnerabilityDecisionEdited` (`vulnerability_decision_edited`), with the decision id, entry, reason and earlier text;
  - relabel `VulnerabilityReopened` to "Vulnerability decision lapsed", keeping its stable name.
- [x] `self_alerts.rs`: add `VULNERABILITY_REPOSITORIES_REF` (`vulnerability-repositories`) with its doc, warn-only, `STALE_AFTER` a day. Its detail names each repository and the failure. Model it on `ANSIBLE_CATALOGUE_*`.

### Jobs (`crates/jobs/src/vulnerabilities`)
- [x] New `repository.rs`:
  - a head watch via smart HTTP `info/refs`;
  - a tree listing at the sha;
  - a raw fetch of each lockfile and the manifests it needs;
  - a `Failure::{Unreachable, Unparseable { lockfile }}` result.
- [x] `lockfile.rs` becomes `lockfile/npm.rs`, keeping dev entries and tagging each edge runtime, development or optional (the old `development_only_packages_are_left_out` test is turned around). Add `lockfile/cargo.rs` (`Cargo.lock` plus member `Cargo.toml`, using the `toml` and `cargo-lock` crates, added with `cargo add`) and `lockfile/uv.rs`. Each returns `Vec<RepositoryPackage>` with the shortest chain per workspace package and scope.
- [x] `scan.rs`:
  - add a repository loop: check each head every 5 minutes, re-read on change, retry failures, store or note the failure;
  - drop the `CountedFleet` fingerprint for the `Counted` set, so a stored change rescans within a minute;
  - one bad lockfile leaves the repository's stored packages untouched;
  - raise or clear the self-alert each pass;
  - on start, withdraw standing `via = model` decisions as `canopy("vulnerability-migration")`, audited, and give each a suggestion with basis insufficient information.
- [x] `ai.rs`:
  - remove `Ground`, `Plan::Decide`, `ground_holds`, `FRONTEND_WORKSPACE` and the decision branch of `look_at`;
  - rewrite `INSTRUCTIONS`: no source access, infer only from structure and advisory text, never claim reachable or unreachable, basis required, may split per workspace package;
  - rewrite `prompt`: advisory; per affected package the workspace packages with scopes, direct flag, shortest chain and minimum fix; Linux gets package and advisory only;
  - schema `{ basis, entries: [{ workspace_packages, outcome, reason }] }`, with an answer that has no basis or fails validation counting as no answer.
- [x] Remove the Tamanu-version lockfile fetch and `versions_unread` uses from `scan.rs`, and `CountedVersion` references in `monitor.rs`.

### Private API (`crates/private-server/src/fns/vulnerabilities.rs`)
- [x] `list` takes `ListQuery`-shaped args and returns items, total, next cursor and facets. `count` (read_only, silent) feeds the sidebar badge.
- [x] `decide` takes entries and drops `review_by`, keeping `write, audit(VulnerabilityDecided)`. Add `withdraw` (`write, audit(VulnerabilityDecisionWithdrawn)`) and `edit` (`write, audit(VulnerabilityDecisionEdited)`), both admin, on `audit.begin()`.
- [x] `just gen-openapi`. Add the new endpoints to `private-web/src/safety-modes.ts` (withdraw and edit as write, count as read-only).

### MCP (`crates/canopy-mcp`)
- [x] `find_vulnerabilities`:
  - takes the filters, `search`, `cursor`, `page_size`, `fields`, `group_by` and `include_details`;
  - identifier and source are always returned;
  - `details` is returned only when named;
  - `group_by` returns groups with counts and no items;
  - the description states that suggestions are made without source.
- [x] `get_vulnerability` returns the new `ItemDetail`, with basis and the no-source note on the suggestion.
- [x] Replace `record_vulnerability_decision` with:
  - `record_vulnerability_decisions` (a batch, each item judged and audited on its own, a result per item in order);
  - `withdraw_vulnerability_decisions`;
  - `edit_vulnerability_decision`.

  All three are triage-grant gated, and `AUDITED_TOOLS` is updated.
- [x] Turn on rmcp prompts. Add a `triage_vulnerabilities` prompt taking `repository` that:
  - lists the items needing triage;
  - groups and orders them for review;
  - reads the code for each vulnerable package;
  - proposes in batches of ten with a reason saying what was checked;
  - records only after the user confirms;
  - treats suggestions as unverified;
  - leaves what can't be checked for a person.

  Update the server instructions' vulnerability paragraph.

### UI (`private-web`)
- [x] `AppSidebar.tsx`: add a count badge on Vulnerabilities from `vulnerabilities/count`, like `IncidentCount`.
- [x] `routes/Vulnerabilities.tsx`, per the list mockup:
  - a search field;
  - a "Show closed" switch replacing "Show all";
  - status (Needs triage default), source, scope, severity and suggestion selects with counts;
  - a result count;
  - sortable headers;
  - server-side paging;
  - all state in URL params;
  - Package, Source, Scope and Status columns ("Untriaged" chip, a "Closed" line, a suggested outcome with its basis, an "Edited/Withdrawn" line), and Machines for Linux items.
- [x] `routes/VulnerabilityDetail.tsx`:
  - an affected-packages table (package and version, reached from, scope, direct, fix in, commit read at);
  - the minimum fix version on Linux machines;
  - a "decision lapsed" banner in place of "Reopened";
  - history entries for edits (with the earlier text struck through), withdrawals and lapses;
  - "AI · based on …" on suggestions.
- [x] `components/VulnerabilityDecisionPanel.tsx`, per the split-decision mockup:
  - a whole/split toggle;
  - per-package outcome rows plus a catch-all;
  - per-row reason and issue link;
  - a coverage line that blocks Record until every package is covered;
  - prefill from the suggestion (whole or split), or from the lapsed decision;
  - Withdraw (with a reason) and Edit actions on a current decision;
  - the review date removed.
- [x] `VulnerabilityChips.tsx`: "Untriaged" for needs triage, and a mixed status.

### Tests
- [x] `crates/jobs` unit tests:
  - npm: a dev-only path is development, an optional dependency is optional, a package reached by two scopes carries both, and the shortest chain is kept;
  - Cargo: build and dev kinds, target-specific tables, transitive inheritance;
  - uv: groups are development, extras are optional, members are found.
- [x] `crates/database/tests/it/vulnerabilities.rs`:
  - the same npm version in two repositories makes two items;
  - a fix landing closes an item and keeps its status;
  - an unreadable repository keeps its items;
  - archiving a machine closes its Linux item without a rescan;
  - split validation (all six refusals);
  - status derives to mixed or the shared outcome;
  - withdraw keeps the suggestion and a lapse discards it;
  - a lapse on a new workspace package for a split without a catch-all, but not with one;
  - an edit of one entry keeps the earlier text;
  - search ranks a summary hit above a details-only hit and tolerates a typo;
  - facets each ignore their own filter;
  - `group_by` scope counts a multi-scope item in each;
  - the cursor pages through without overlap.
- [x] `crates/jobs/tests/it/vulnerabilities.rs` (fake GitHub with `info/refs`, tree and raw routes):
  - reads on a head change only;
  - one bad lockfile leaves the stored packages and raises the self-alert, and a good read clears it;
  - model decisions are withdrawn on start with an audit;
  - the model never decides;
  - an answer with no basis or an invalid split gives no suggestion;
  - a valid split suggestion is stored;
  - a Linux prompt has no machine names.
- [x] `crates/private-server/tests/it/vulnerabilities.rs`: a split decision is recorded and audited with entries; withdraw and edit are audited and admin-only; a read-only session refuses all three; the list paging and search arguments round-trip.
- [x] `crates/public-server/tests/it/mcp.rs`:
  - a batch records valid items and refuses invalid ones in order, auditing only the recorded;
  - withdraw and edit through a granted token, with a token without the grant refused;
  - `fields` limits the output;
  - `group_by` returns counts only;
  - the prompt is listed and takes `repository`.
- [x] e2e: rewrite `seedTamanuDependencies` as `seedRepositoryPackages`, then cover in `e2e/vulnerabilities.spec.ts`:
  - the sidebar badge;
  - search with highlight;
  - filter counts and the result count;
  - sorting a column;
  - the closed switch;
  - recording a split and seeing Mixed;
  - withdrawing and editing in the history;
  - a lapsed banner.

### Finish
- [x] `just gen-openapi`, `just check`, and targeted `just test -p <crate> vulnerab`.
- [x] Check `grep -rn 'review_by\|Ground\|tamanu' crates/*/src/vulnerabilities* crates/canopy-mcp/src/vulnerabilities.rs private-web/src` for leftovers.
