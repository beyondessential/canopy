# Vuln tracking improvements: plan notes

## Data carried over

- Existing `tamanu` items, matches and decisions become items and decisions of the Tamanu repository source; the source column moves from a two-value enum to a source name (a repository name, or `linux`).
- Review dates on existing accepted risks are dropped and those decisions stand.
- Decisions the model made (via `model`) are withdrawn by the migration and become suggestions, with a basis of insufficient information where none was recorded, so a person looks at each.
- Existing decisions are whole decisions.
- Per-version Tamanu lockfile tables (`version_lockfiles`, `version_packages`) give way to per-repository package tables read at the commit of `main`.

## Notes

- The repository list is hardcoded for now; moving it to the database with an edit UI is W5. Scanning container images in clusters is explored in X5.
- Scope vocabulary follows SPDX 3 lifecycle scopes (runtime, build, development, test, design, other), with `optional` as a separate flag. npm supplies runtime/development/optional from the lockfile; Cargo supplies runtime/build/development from manifest dependency kinds. uv supplies runtime, development (dependency groups) and optional (extras) from `uv.lock`.
- Only the shortest chain per workspace package and scope is stored, since npm trees can have very many paths to one package.
- Closed is a flag beside triage status, not a status value; the list's "show closed" switch and MCP's include-closed flag both drive it.
- Repositories are public and read without credentials, as infra's catalogue is.
- Search wants a trigram index (pg_trgm) over identifier, aliases, summary, package, workspace packages and source, with details weighted lower.

## Checklist

- [ ] To be drafted once specs are agreed and mockups done
