---
status: draft
---

# Audit log of administrative actions

A systematic record of who did what in Canopy, captured at the middleware or extractor level for every administrative action, with write and danger entries replicated to the backups as an append-only offsite copy.

## Context

- The safety-mode gate (T2, spec SAFE) is shipped. Every handler on the administrative surface declares a grade (read-only, write, danger, plus danger reasons) as an OpenAPI extension, and one middleware (`crate::safety::enforce`) reads the grade back by matched method and path and decides each request against the caller's session. That middleware already knows the actor, the session, the session's mode, the operation and its grade, so it is the natural capture point.
- Per-feature provenance exists as roughly twenty `*_by` columns (`revoked_by`, `changed_by`, `resolved_by`, `held_by`, `restore_allowed_by`, …), plus the cached tailnet user table for avatars. Backup schedule layers keep their own change history (BKO).
- An events table existed and was dropped (`2026-07-08…_drop_events`).
- A5 (Canopy as an OIDC identity provider) waits on this card and wants its grant changes and token issuance to live here rather than in tables of its own. Its needs, from the comment on this card:
  - device actors as well as operators (a machine acting for itself or one of its applications)
  - volume in the tens of thousands of entries a day, stored in time ranges with retention of at least 30 days, the way history storage (HST) works
  - writing as a condition of the action: an entry written inside the action's transaction, able to fail it
  - refusals recorded with their reason
  - lookup by fields in an entry's detail, and counts per field over a window (key rotation counts uses per signing key)
  - identifiers only, never the credential
  - grants and revocations logged with who and when, so the feature carries no who/when columns
- The card branch is well behind main (the SAFE spec and the grading middleware are on main only). Rebase before implementing.

## Behaviour

### What is logged

- Every request on the administrative surface graded write or danger produces an entry.
- Read-only requests produce one only where the handler is marked as a sensitive read (the SQL playground is the obvious first; the full list is open).
- Actors are operators (tailnet users), devices (a machine acting for itself or one of its applications), fleet-query access tokens, and Canopy itself (background work that changes things on its own).
- Device requests produce an entry when they issue a credential or trust material (tokens, backup and restore credentials, certificates) or change Canopy's records beyond reporting status (enrolment, for instance). Status pushes and reads do not.
- Canopy's own background work produces an entry only where a job opts in.
- Fleet-query tool calls follow the same rule as the administrative surface: all its tools are read-only, so only those marked as sensitive reads are logged, with the access token as the actor.
- Authorisation refusals are logged: a caller who is not an administrator reaching an administrative handler, and a device refused something it asked for (A5's refused token requests, with the reason).
- Safety-mode refusals and actions that were permitted but errored are not logged.

### What an entry holds

- The actor, when, and the operation.
- Of the request's arguments, only those the handler declares as recorded. Nothing is recorded by default, so a secret reaches the log only by someone declaring it. The declaration sits beside the handler's grade, the same place and the same way.
- Canopy's routes are mostly `POST /api/<module>/<fn>` with a JSON body, so the target of an action (which machine, which configuration) lives in the arguments too. The declared summary is what makes an entry say what it acted on.

### Writing as a condition of the action

- An action whose entry cannot be written is not carried out. This holds for every logged action, not only danger, and matches what A5 needs for token issuance.

## Implementation options

## Open questions

- [ ] Fail-closed with generic capture. The middleware sits outside the handler's transaction, so "no entry, no action" cannot come from sharing a transaction without touching handlers. Candidates: write the entry ahead of the handler and refuse if that fails, then settle it once the handler returns (which leaves an entry of unknown outcome if Canopy dies mid-request, arguably correct); or hand handlers a transaction-scoped recorder through an extractor (which edits handlers, the thing the card wants to avoid). Write-ahead also produces entries for actions that then error, which the decision above says are not logged, so they would be removed or settled as not carried out.

## Trade-offs

## Testing notes
