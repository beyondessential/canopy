# J6: notes history, note writes over MCP, incident memory

Builds what these specs describe: [FLT](../../specs/servers/overview.md) "Notes", [INC](../../specs/monitoring/incidents.md) and [CHK](../../specs/monitoring/checks.md) notes, [MEM](../../specs/monitoring/incident-memory.md), and the new Notes and Incident memory sections of [MCP](../../specs/private-server/mcp.md).
Four layers, in order: schema and database crate, operator API, MCP tools, operator UI.
Each section ends in its own targeted tests; run them by filter once a section's commits are in, not per commit.

## Design notes

- **Notes log for group, machine, application.**
  One `notes_revisions` table, one nullable FK column per grain (`server_group_id`, `machine_id`, `application_id`) with a `num_nonnulls = 1` CHECK and `ON DELETE CASCADE`, as the check-state scope columns do.
  Each row holds the notes text *after* the change, the revision number, who (nullable, for the backfill), how (`operator`, `ansible_import`, `token`), and when.
  The record carries `notes_revision` (latest number).
  Every writer goes through one helper that locks the row, compares, writes the row and bumps the number, so nothing writes `notes` directly any more.
  That includes the `notes: Option<String>` fields of the group, machine and application update changesets, the create paths, and the Ansible import's appends.
  A save equal to the current text writes nothing and records no audit entry.
- **Existing notes are backfilled** as revision 1 with no author, so every non-empty `notes` has a log.
- **Issue and incident notes.**
  Add `author_kind` (`operator` or `agent`), `revision`, `deleted_at`, `deleted_by` to both note tables.
  Edits keep the replaced text in a per-kind `*_note_revisions` table (mirroring the existing per-kind duplication).
  Existing rows backfill as `operator`.
  Delete becomes an update setting `deleted_at`; every list query filters it out unless asked.
  `slack_outbox.note_id` cascades on note delete today; soft delete means it no longer fires, so check what the outbox does for a note that was edited or deleted after its message was queued.
- **Memory.**
  `memory_entries` holds title, body, at most one link (`server_group_id`, `incident_id`, or `check_source` + `check_name`, held to one grain by a CHECK), `revision`, created and updated attribution, and retirement (`retired_at`, `retired_by`, `retired_reason`).
  `memory_entry_revisions` holds each version's content, its kind (`recorded`, `amended`, `retired`, `restored`), actor, how, reason, and time.
  Link FKs are `RESTRICT`, not `SET NULL`, so an entry never silently turns fleet-wide.
  Confirm how groups and incidents are removed first; a group is archived, so this should never bite.
  Search uses a tsvector GIN index over title and body, prefix-matching as the vulnerability search does.
- **Stale writes** use the existing conflict error (409) in the operator API and a tool error carrying the current text and revision in the MCP tools.
- **Audit.**
  New catalogue actions: notes changed (record kind in detail, revision), issue note added, incident note added, note edited, note deleted, memory entry recorded, amended, retired, restored.
  Detail never carries note or memory text, only the record and revision.
  Memory entries and notes need a `RecordKind` each so entries name what they acted on.
- **Operator edit of issue and incident notes** is not specced, so the operator UI gets add and delete only; revisit if the open question on the card is answered the other way.
- **Slack outbox** does not queue note messages today, so soft delete has nothing to reconcile there.

## Schema and database crate

- [x] `just migration notes_history`: `notes_revisions`, `notes_revision` columns on `server_groups`, `machines`, `applications`, backfill of revision 1 for non-empty notes
- [x] `just migration note_edit_and_delete`: `author_kind`, `revision`, `deleted_at`, `deleted_by` on `issue_notes` and `incident_notes`; `*_note_revisions` tables
- [x] `just migration incident_memory`: `memory_entries`, `memory_entry_revisions`, indexes, full-text index
- [x] Add `database::notes_log` with `set(conn, record, new_text, actor, via, expected_revision: Option<i64>)` and `history(conn, record)`; the stale case returns the conflict error with the current text and revision
- [x] Route `ServerGroup`, `Machine` and `Application` create and update through it and remove `notes` from their changesets (`server_groups.rs`, `machines.rs`, `applications.rs`)
- [x] Route the Ansible import's group and machine notes appends through it with `ansible_import` as how (`fns/ansible/import.rs`)
- [x] Extend `IssueNote` and `IncidentNote` in `notes.rs`: author kind on add, `edit` (revision-guarded, keeps the replaced text), soft `delete`, `include_deleted` on the list functions, edit history read; update the module docs, which say notes are immutable
- [x] Add `database::memory`: `record`, `amend`, `retire`, `restore` (each revision-guarded where the spec says so), `get` with history, `find` (free text, link kind, group, check, incident, relevant-to-incident, include retired, limit, total), and `relevant_count_for_incident`
- [x] "Relevant to an incident" is entries linked to the incident, to its target's group (through `Scope`/`IncidentTarget`, never the raw columns), or to a check among its issues' check names
- [x] Add `database::memory::index(group, window_days)`: current entries in compact form; checks and groups with the most incidents in the window with incident, published, latest, group and linked-entry counts; incidents in the window carrying non-deleted notes. Reuse the published rule and window rule `Find incidents` uses rather than restating them
- [x] Add the audit actions and `RecordKind`s in `crates/database/src/audit.rs`
- [x] Add `GRANT_NOTES = "notes"` to `mcp_tokens.rs` `GRANTS`
- [x] Database tests: revision numbering and no-op save, stale refusal, backfill, one-grain CHECKs, soft delete hiding from lists, edit keeping history, memory lifecycle including restore of a retired entry, search and relevant-to-incident, index counts and the gap rows (`just test -p database <filter>`)

## Operator API (`crates/private-server`)

- [x] `fns/issues.rs` and `fns/incidents.rs`: `add_note` audited with author kind `operator`; `delete_note` soft and audited; `list_notes` takes `include_deleted`, honoured for administrators only; note data gains author kind and deleted attribution; fix the doc comments that say deletion is permanent
- [x] `fns/server_groups.rs`, `fns/machines.rs`, `fns/applications.rs`: change `update` (and creates) from `silent` to `audit_optional(NotesChanged)` and record on a real notes change; add `notes_history` reads for each grain
- [x] `fns/ansible/import.rs`: declare and record notes changed beside the import's own action
- [x] New `fns/memory.rs`, mounted at `/api/memory`: `list` and `get` for any tailnet user (with `total`, filters as in the spec), `record`, `amend`, `retire`, `restore` for administrators, each audited
- [x] `fns/mcp_tokens.rs`: accept the notes grant on minting and list it
- [x] Update `ERRORS.md` if any new error variant is added; prefer the existing conflict variant
- [x] `just gen-openapi`, commit `private-web/openapi.json` and `api-types.ts`
- [x] Endpoint tests in `tests/it/` (`notes.rs` extended, new `memory.rs`): admin-only writes, non-admin reads, audit entries present with no text in detail, soft delete, history reads. Add a debug-shortcut-free denial case following `admin_auth.rs`

## MCP (`crates/canopy-mcp`)

- [x] `caller.rs`: `may_note()` beside `may_triage()`, an administrator on the operator path, the notes grant on the token path
- [x] New `notes.rs` module: `add_issue_note`, `add_incident_note` (author kind `agent` for a token, `operator` for an operator caller), `edit_note`, `delete_note` (refused for an operator-written note, naming the reason), `replace_notes`, `get_notes_history`; stale revisions return the current text and revision
- [x] `get_group`, `get_machine`, `get_application` return notes with their revision; `get_issue` and `get_incident` return notes with author, author kind, revision and time, never deleted ones (`groups.rs`, `machines.rs`, `applications.rs`, `incidents.rs`)
- [x] New `memory.rs` module: `find_memory`, `get_memory_entry`, `get_memory_index`, `record_memory_entry`, `amend_memory_entry`, `retire_memory_entry`, `restore_memory_entry`; describe the index's lists and the gap rows in the tool descriptions so a client reads what it is for
- [x] Add the new write tools to `AUDITED_TOOLS` in `lib.rs`, register both routers in `CanopyMcp::new`, and update the module docs, which say only the three triage tools write
- [x] Update the token mint dialog and token list so the notes grant is chosen and shown (`private-web/src/routes/McpTokens.tsx`)
- [x] Tests: grant gating on both paths, each tool's refusal cases (stale revision, operator-written note, over-length title or body, retired entry), the audited-tool test still holding, and the index's counts against seeded incidents

## Operator UI (`private-web`)

- [x] Notes history on group, machine and application pages: a history view newest first showing each revision's text, author, how, and when (`GroupDetail.tsx`, `MachineDetail.tsx`, `ServerDetail.tsx`); the edit forms keep working unchanged
- [x] `NotesList.tsx`: administrators get a show-deleted toggle; deleted notes render with who and when; agent-written notes are marked as such
- [x] New memory page under the Incidents navigation: search box, link kind and group/check/incident filters, include-retired toggle, all held in the query string; entry detail with resolved link and history; administrators get record, amend, retire and restore (`routes/Memory.tsx`, route in `App.tsx`)
- [x] `IncidentDetail.tsx`: relevant-entries count linking to the memory page with the `relevant to incident` filter in the query string
- [x] Playwright: `memory.spec.ts` (list, search, filters from the address, admin-only controls, retire and restore), notes history specs added to `groups.spec.ts` and `machines.spec.ts`, deleted notes in `issues.spec.ts` or `incident-*`, the notes grant in `mcp-tokens.spec.ts`; extend `e2e/seed.ts` for memory entries and revisions. Run individual spec files only
- [x] `just typecheck`

## Wrap-up

- [x] `cargo fmt`, `just check`, no warnings in tests or main code
- [x] `grep -rin deployment` over the touched specs and code stays clean
- [ ] Delete this plan in an `unplan:` commit once the work has shipped
