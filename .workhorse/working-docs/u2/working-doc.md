---
status: complete
---

# Audit log of administrative actions

A systematic record of who did what in Canopy, captured at the middleware or extractor level for every administrative action, archived offsite in non-overlapping periods once it ages out of the working set.

## Context

- The safety-mode gate (T2, spec SAFE) is shipped. Every handler on the administrative surface declares a grade (read-only, write, danger, plus danger reasons) as an OpenAPI extension, and one middleware (`crate::safety::enforce`) reads the grade back by matched method and path and decides each request against the caller's session. That layer already resolves the actor and the matched operation, so it is a candidate capture point. Only the place is shared: nothing about grades or modes goes into the log.
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
- Several specs already say actions "are audited", ahead of any log existing: maintenance windows, Kubernetes sleep and wake, inventory leases and variables (with inventory reads logged with their intent), upgrade plans, restore-replica declarations and issuances, reporting-schema declarations, and the escrow recovery ceremony. Each is a handler that declares itself audited once this lands.
- Feature-specific audit tables exist alongside the who columns: `backup_credential_issuances` (every STS issuance to a device) and `backup_schedule_history` (shown in the schedule editor, BKO). `sql_playground_history` is an operator convenience rather than an audit record.

## Behaviour

### What is logged

- Whether a request produces an entry is the handler's own audit declaration, and nothing else. Every handler makes one, either audited (with its recorded arguments) or audit-silent, and a handler that declares neither fails the build, so being silent is a deliberate choice. It is orthogonal to the handler's safety grade: a read can be audited (the SQL playground, say) and a write can be audit-silent.
- Safety modes are a guard on the operator's client against mistakes, not an audit concept, so an entry carries no grade, mode, or session.
- Actors are operators (tailnet users), devices (a machine acting for itself or one of its applications), fleet-query access tokens, and Canopy itself (background work that changes things on its own).
- On the public server, the handlers that issue a credential or trust material (tokens, backup and restore credentials, certificates) or change Canopy's records beyond reporting status (enrolment, for instance) declare themselves audited. Status pushes and reads do not.
- Canopy's own background work produces an entry only where a job opts in.
- Fleet-query tools declare themselves audited or not in the same way, and an audited call is logged with the access token as the actor.
- Authorisation refusals are logged: a caller who is not an administrator reaching an administrative handler, and a device refused something it asked for (A5's refused token requests, with the reason).
- Actions that were permitted but errored are not logged.

### What an entry holds

- The actor, when, and the operation.
- Of the request's arguments, only those the handler declares as recorded. Nothing is recorded by default, so a secret reaches the log only by someone declaring it. This is the same declaration that makes the handler audited.
- Canopy's routes are mostly `POST /api/<module>/<fn>` with a JSON body, so the target of an action (which machine, which configuration) lives in the arguments too. The declared summary is what makes an entry say what it acted on.

### Writing as a condition of the action

- An action whose entry cannot be written is not carried out. This holds for every audited action, and matches what A5 needs for token issuance.

### Archive

- The offsite store is the log's archive, not a copy of it. Postgres holds the working set; once an entry is safely in the archive it can be removed from Postgres.
- The archive is written in non-overlapping periods: each archived object covers one closed span of time, spans abut, and every entry is in exactly one of them. No entry is archived twice, so there are never N copies to reconcile.
- Every entry is archived, whoever the actor.
- A period is safe once its object has been written, read back and checked against what was sent, and is under object lock. Only then may its entries leave Postgres.
- The archive lives in the recovery escrow's storage (ESC), which is already versioned and object-locked, under its own prefix. Archived entries are not encrypted to the escrow recipients, since Canopy reads them back.
- The working set keeps 90 days. An entry leaves Postgres only once it is both archived and older than that. This covers A5's floor of 30 days for its per-key counts and lookups.
- Some entry types, low in volume and high in importance, stay in the working set for a year. The handler's audit declaration says so.
- Archived periods are kept forever. Object lock covers the escrow storage's required retention, and the objects stay after it expires.
- An operator can ask for entries older than the working set, and Canopy fetches the archived periods covering them.
- To be raised as an entry in this card's breakdown.

### Self-alerts

- The log's ranges are provisioned ahead like the other histories (HST), with the same self-alert as they run short: warning below two weeks of future range, failure below one. Running out stops every audited action, since writing is a condition of the action.
- Archiving falling behind raises a self-alert (SELF): a warning when a closed period has gone unarchived past a delay, and a failure when the working set has grown past what it is meant to hold.

### Reading the log

- Any administrator can read every entry.
- A log page lists entries newest first, filterable by actor, operation, time and target. Reaching past the working set fetches from the archive (above).
- The fleet query interface can read entries too, limited to the working set.

### Existing provenance

- The per-feature who columns (`revoked_by`, `held_by`, `resolved_by`, …) are replaced: features read who from the log, and the columns are dropped. The when columns stay, as part of the record's own state. To be raised as an entry in this card's breakdown.
- The feature-specific audit tables fold in the same way: backup credential issuances and backup schedule history become log entries and the tables are dropped. The schedule editor shows history from the log, so within the working window plus any entry type kept longer. Part of the same breakdown entry.
- A page shows who did something only while the entry is in the working set. Past that it shows when, and the log page (reaching into the archive) answers who.

## Implementation options

- History storage already keeps high-volume tables in weekly ranges (HST). Storing the log the same way lines archive periods up with Postgres ranges: archive a whole closed week as one object, confirm it landed, then drop that week's range. The period boundary is the range boundary, so non-overlap falls out of the storage rather than needing tracking per entry, and removing a week is a cheap drop rather than a bulk delete.
  - Longer-lived entry types break the whole-range drop, since a week's range holds both kinds. Either keep them in a separate table, which can have its own ranges or none at all given the low volume, or move them out of a range before it is dropped.

### Capture: an extractor carrying the audit transaction

Decided: the entry is written inside the handler's own transaction, handed to it by an extractor. The entry and the change commit together, and an action that errors rolls back and leaves no entry, which is what "errored actions are not logged" asks for without any settling or deleting.

- The `routes!` declaration (`audit(ArgsType)` or `silent`) is recorded as an OpenAPI extension and read back at startup into an audit map, as grades are.
- Recorded fields are declared on the handler's argument type: a derive, with `#[audit]` on each field to record. The `routes!` clause names that type, so a renamed or removed field is a compile error, and the recorded names are the ones serde reads, renames included. A handler with no arguments, or nothing worth recording, names no type and records only the operation.
- An audit layer, a sibling of `safety::enforce`, buffers the request body, picks the declared fields out of it, and leaves the actor, operation and recorded fields in the request's extensions. The extractor is `FromRequestParts`, so it cannot read the body itself while `Json<Args>` consumes it; the layer does that part.
- The extractor checks out a primary connection, begins a transaction, writes the entry, and hands the handler the connection. The layer keeps a handle to the same transaction and commits it on a successful response, rolling it back otherwise.
- Audited handlers swap `state.db.get()` for the extractor. Mechanical, explicit at the call site, and the handler can see it is inside the audit transaction.
- An audited read takes the extractor too, so the entry goes to the primary even where the read would otherwise use `db_read`.
- Public-server handlers (device issuance, A5) take the same extractor with the device as actor. Jobs that opt in call the recorder directly inside their own transaction with Canopy as actor.
- Authorisation refusals happen in `TailscaleAdmin` (or device auth) before the handler, so no transaction exists; the layer writes the refusal entry on its own.
- The `routes!` audit clause asserts at compile time that the handler takes the audit extractor, through a trait implemented across extractor-tuple positions the way axum's `all_the_tuples!` does for `Handler`. An audited route whose handler forgets it fails the build, like a missing declaration.
- A handler that acts outside Postgres (S3, Kubernetes, STS) commits its entry through the extractor before the outside effect, as pending. Once the effect is confirmed, the handler's transaction records the confirmation along with its own changes. An entry that never confirms stays pending, and the log page shows it as unconfirmed, because the outside effect may have happened even though the request failed. This is the one case where an action that errors leaves an entry.
- A confirmation is appended as its own record pointing at the pending entry, rather than an update to it, so the log stays append-only and an archived period never needs rewriting. A pending entry and its confirmation can fall in different periods.
- Cost: an audited request is one transaction, held across whatever the handler does, and a handler cannot commit part of its work before failing.

### Storage

- One table, partitioned by week like the other histories, and provisioned ahead through `database::partitions`. Each week is sub-partitioned by retention class, so both removals are a partition drop rather than a bulk delete. At 90 days, once the week is archived, the short-lived sub-partition is dropped. At a year, the rest of the week goes too.
- An entry row holds when it happened, the actor (a kind, being operator, device, fleet-query token or Canopy, plus its identifier), the operation, a kind (action, refusal, or confirmation of a pending action), and a `jsonb` detail with the recorded fields, a refusal's reason, or the pending entry a confirmation points at. A GIN index over the detail serves A5's lookups by token id, signing key, subject and relying party, and its per-key counts over a window.

### Archive

- One Parquet object per closed week, with every retention class in it. Columnar, so a read back for the log page skips the columns and row groups it does not need.
- Periods are hash-chained to each other in the archive: each object records the hash of the previous period's object, so a missing or replaced period shows up as a break. Entries in the working set are not chained. Writes interleave freely there instead of being forced into a single order, and a chain in Postgres would not stop anyone able to write to Postgres anyway, since they could rewrite the chain too.
- The archiver runs in the backups pod, which already holds the escrow role. The private server fetches archived weeks for the log page under a separate role that can only read the audit prefix. Both roles are granted on the ops side, in the escrow bucket's account.
- The archiver writes a week's object, reads it back and checks it against what it sent, and confirms the object lock before marking the week archived. Only then can a drop touch that week.

### Anchors in the code

- `routes!` (vendored `canopy-utoipa-axum`) takes the grade as a prefix, `routes!(danger(fleet, invalidates): revoke)`, records it as the `x-canopy-safety-mode` OpenAPI extension, and fails to compile without one. `safety::GradeMap` reads it back from the built document at startup. An audit declaration can ride the same macro as its own extension, read back into its own map, with no second copy anywhere.
- `safety::enforce` is a route layer that already resolves the tailnet identity and the matched path. The audit layer is a sibling, not part of it, since the two concerns are orthogonal; it can sit next to it in the stack.
- `database::Db` wraps the mobc pool precisely so checkout can change "without changing any of the call sites"; handlers make about 180 `db.get()` calls between them and almost none open their own transaction. That is what would make a request-scoped transaction possible without editing handlers.
- Partitioned histories have `database::partitions::{ensure_runway, runway}` and the HST self-alert; the log joins them.
- The recovery escrow writes `canopy-recovery/state.age` to a bucket in a separate account through an assumed role, write-only by design. Reading archived periods back needs `GetObject` on the audit prefix granted to some role Canopy holds, which is an ops-side change in that account. The escrow snapshot also carries the backup schedule history (version 3); once that history folds into the log, the snapshot reads it from the log or drops it.

## Open questions





## Trade-offs

- Temporal tables for state history, rejected. With the deployment on PG18, `WITHOUT OVERLAPS` keys could hold schedule layers, A5's grants and the various windows as state over time, leaving the log to say only who. But the log is not a store for history: it is the record of what was done, written as a condition of doing it and archived offsite under lock. Moving the changes themselves into temporal tables would take them out of that record. Schedule history folds into the log as decided.
  - For reference: Postgres offers application-time periods only. PG18 has `WITHOUT OVERLAPS` keys and `PERIOD` foreign keys; PG19 adds a documentation section and `range_minus_multi()`; `UPDATE`/`DELETE … FOR PORTION OF` was reverted from 19 and the development branch on 15 September 2026. Neither has system-versioned tables. A feature that wants "never two at once" as a constraint can still use `WITHOUT OVERLAPS` on its own state; that is separate from auditing.

## Testing notes

- A handler with no audit declaration fails the build; one declared silent produces no entry whatever its grade.
- An audited read (SQL playground) produces an entry; an audit-silent write produces none.
- An entry holds only the declared arguments; an undeclared secret argument never appears in it.
- An entry carries no grade, mode or session.
- An audited action whose entry cannot be written is refused and changes nothing.
- An audited action that errors leaves no entry.
- A non-administrator reaching an audited administrative handler produces a refusal entry; a safety-mode refusal produces none.
- A device credential issuance produces an entry with the device as actor; a status push does not.
- A closed period is archived as one object, read back, checked and locked, and only then leaves Postgres, and only once past 90 days.
- Archived periods never overlap: re-running the archiver over an archived period writes nothing new.
- The log page reaches past the working set into the archive; the fleet query interface does not.
- A page shows who did something within the working window, and only when after it.
- Running short of ranges, and archiving falling behind, each raise their self-alert and clear once fixed.
