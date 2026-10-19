# Audit log: plan

Implements [AUD](../../specs/platform/audit-log.md).
The reasoning behind these choices is in the working doc (`.workhorse/working-docs/u2/working-doc.md`).

This card builds the log itself: declarations, capture, storage, the log page, the fleet query tool, and the range self-alert.
Two pieces are split into their own cards (see the breakdown): the offsite archive, and replacing per-feature provenance with the log.
Until the archive lands, nothing leaves the database, which AUD already requires of an unarchived week.

## Actions

- One `database::audit::Action` enum is the catalogue: a variant per action, each carrying its typed detail and the records it acted on (typed references, not free ids). Its serde tag is the stable action name, independent of route paths, so renaming a handler or its route leaves the name alone.
- Retention is a method on the action, ninety days by default and a year for the few marked.
- A variant has no field for a secret, so keeping secrets out is a property of the type, not of what someone remembered to mark.
- Jobs, device API handlers and operator handlers record the same variants.

## Declaring

- `routes!` (vendored `canopy-utoipa-axum`) gains an audit clause beside the grade: `audit(ActionA, ActionB)` naming the action variants the handler may record, or `silent`. A route with no audit clause fails to compile, as a missing grade does.
- The clause is recorded as its own OpenAPI operation extension and read back at startup into an audit map, the way `safety::GradeMap` reads grades.
- The audit clause asserts at compile time that the handler takes the audit extractor, through a trait implemented across extractor-tuple positions the way axum's `all_the_tuples!` implements `Handler`.

## Capture

- An audit layer, a sibling of `safety::enforce` rather than part of it, resolves the actor and leaves it in the request's extensions. It reads no body.
- The extractor checks out a primary connection, begins a transaction, and hands the handler the connection and a recorder. The handler records `Action` values on it inside that transaction. The layer holds the same transaction and commits on a successful response, rolling back otherwise. Audited handlers replace `state.db.get()` with it; audited reads use it in place of `db_read`, since the entry is a write.
- On a successful response the layer checks that at least one action was recorded and that every recorded action is one the route declares. Otherwise it rolls back and returns an error, so a forgotten record fails the request and any test of the route.
- The recorder can commit an entry ahead of the handler's transaction, on its own connection, so it stands whatever the handler's transaction does. A handler with an outside effect uses it to record its action as pending before the effect, then appends the confirmation in its own transaction afterwards. A feature that records refusals (A5) uses it to record the refusal before returning the error.
- Device API handlers take the same extractor with the device as actor. Jobs record through the same recorder inside their own transaction with Canopy as actor.
- Cost, accepted: an audited request is one transaction held across everything its handler does, and a handler cannot commit part of its work before failing.

## Storage

- One table, partitioned by week through `database::partitions` and provisioned ahead with the other histories, so the HST runway alert covers it with no new code beyond registering the table.
- Each week is sub-partitioned by retention class, so the archive card's removals are partition drops: the ninety-day part once the week is archived and old enough, the rest at a year.
- Row: timestamp, actor kind and identifier, action name, entry kind (action, pending, confirmation), the records acted on, and a `jsonb` detail holding the action's fields, or the pending entry a confirmation points at. The records acted on are indexed for the subject filter; a GIN index on the detail serves lookups and per-value counts.

## Reading

- The log page is a new route on the operator UI, readable by any administrator, filtering by actor, action, time, and record acted on. Reaching past ninety days calls into the archive once the archive card lands; until then the database is all there is.
- Actor icons follow the org-wide convention for action logs: humans in colour, everything else greyscale. An operator shows their profile picture, or an initial on a colour derived from their login where there is none (`UserAvatar` currently falls back to neutral grey). Devices, access tokens and Canopy itself get greyscale icons, Canopy's logo desaturated.
- The fleet query interface gains a "Find audit entries" tool over the database only, open to every caller of the interface.

## Dependencies outside the repo

- Two roles on the escrow bucket's account: write for the archiver in the backups pod, and read-only on the audit prefix for the private server. Needed by the archive card.

## Rejected

- Recording the request as the entry, with declared argument fields as its detail. Routes are renamed while the archive is kept forever, one request can be several actions or none, and the request holds neither what the handler produced (issued credential ids, created records) nor what it read (a lease's intent). The working doc's reasoning predates this and still describes it.

- Temporal tables for state history. The deployment runs PG18, so `WITHOUT OVERLAPS` is usable, but moving changes into temporal tables would take them out of the record the log is. Postgres 19 adds nothing here: `FOR PORTION OF` was reverted before release.
- Write-ahead entries settled after the handler, and a request-scoped transaction through `Db::get()`. The first leaves entries for actions that errored; the second hides the transaction from handlers that hold two connections or spawn tasks.

## Build

- [ ] `Action` enum with stable names, typed records acted on, and retention per action
- [ ] Audit clause in `routes!`: `audit(Action, …)`, `silent`, compile error when missing
- [ ] Compile-time check that an audited handler takes the extractor
- [ ] Layer check: a successful audited response recorded at least one action, all declared
- [ ] Audit map read from the OpenAPI document at startup
- [ ] Migration: partitioned entries table with retention sub-partitions, GIN index; registered with partition runway
- [ ] Database model: write entry, commit an entry ahead, write pending and confirmation, query with filters, count by detail value over a window
- [ ] Audit layer and extractor on the private server
- [ ] Declare every private-server handler audited, with its actions, or silent; the specs already naming audited actions (MNT, K8S, INV, UPG, RST, RPT) are audited
- [ ] Device API: audit issuance and record-changing handlers, silence status and reads
- [ ] Fleet query interface: declare each tool, add "Find audit entries"
- [ ] Recorder for jobs
- [ ] Log page in `private-web/`, with Playwright coverage
- [ ] `just gen-openapi`, and `just gen-api` if the public API's schema changes
