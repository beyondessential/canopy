# Audit log: plan

Implements [AUD](../../specs/platform/audit-log.md).
The reasoning behind these choices is in the working doc (`.workhorse/working-docs/u2/working-doc.md`).

This card builds the log itself: declarations, capture, storage, refusals, the log page, the fleet query tool, and the range self-alert.
Two pieces are split into their own cards (see the breakdown): the offsite archive, and replacing per-feature provenance with the log.
Until the archive lands, nothing leaves the database, which AUD already requires of an unarchived week.

## Declaring

- `routes!` (vendored `canopy-utoipa-axum`) gains an audit clause beside the grade: `audit(ArgsType)`, `audit` with no type for a handler with nothing to record, or `silent`. A route with no audit clause fails to compile, as a missing grade does.
- The clause is recorded as its own OpenAPI operation extension and read back at startup into an audit map, the way `safety::GradeMap` reads grades. Nothing else holds a second copy.
- Recorded fields are declared on the argument type with a derive and `#[audit]` on each field. The clause names the type, so the names recorded are the ones serde reads, renames included, and a renamed field is a compile error.
- Retention is part of the clause (a year-long marker); the default is ninety days.
- The audit clause asserts at compile time that the handler takes the audit extractor, through a trait implemented across extractor-tuple positions the way axum's `all_the_tuples!` implements `Handler`.

## Capture

- An audit layer, a sibling of `safety::enforce` rather than part of it, buffers the request body, picks out the declared fields, and leaves the actor, operation and recorded fields in the request's extensions. It has to do the body work because the extractor is `FromRequestParts` and `Json<Args>` consumes the body.
- The extractor checks out a primary connection, begins a transaction, writes the entry, and hands the handler the connection. The layer holds the same transaction and commits on a successful response, rolling back otherwise. Audited handlers replace `state.db.get()` with it; audited reads use it in place of `db_read`, since the entry is a write.
- A handler with an outside effect calls the extractor to commit a pending entry before the effect, and appends the confirmation in its own transaction afterwards.
- Refusals happen in `TailscaleAdmin` and device auth, before any handler transaction, so the layer writes the refusal entry on its own connection.
- Device API handlers take the same extractor with the device as actor. Jobs call the recorder directly inside their own transaction with Canopy as actor.
- Cost, accepted: an audited request is one transaction held across everything its handler does, and a handler cannot commit part of its work before failing.

## Storage

- One table, partitioned by week through `database::partitions` and provisioned ahead with the other histories, so the HST runway alert covers it with no new code beyond registering the table.
- Each week is sub-partitioned by retention class, so the archive card's removals are partition drops: the ninety-day part once the week is archived and old enough, the rest at a year.
- Row: timestamp, actor kind and identifier, operation (matched method and path), entry kind (action, refusal, confirmation), and a `jsonb` detail holding the recorded fields, a refusal's reason, or the pending entry a confirmation points at. A GIN index on the detail serves lookups and per-value counts.

## Reading

- The log page is a new route on the operator UI, readable by any administrator, filtering by actor, operation, time, and recorded-argument value. Reaching past ninety days calls into the archive once the archive card lands; until then the database is all there is.
- The fleet query interface gains a "Find audit entries" tool over the database only, open to every caller of the interface.

## Dependencies outside the repo

- Two roles on the escrow bucket's account: write for the archiver in the backups pod, and read-only on the audit prefix for the private server. Needed by the archive card.

## Rejected

- Temporal tables for state history. The deployment runs PG18, so `WITHOUT OVERLAPS` is usable, but moving changes into temporal tables would take them out of the record the log is. Postgres 19 adds nothing here: `FOR PORTION OF` was reverted before release.
- Write-ahead entries settled after the handler, and a request-scoped transaction through `Db::get()`. The first leaves entries for actions that errored; the second hides the transaction from handlers that hold two connections or spawn tasks.

## Build

- [ ] Audit clause in `routes!`: `audit(Type)`, `audit`, `silent`, retention marker, compile error when missing
- [ ] Derive for recorded fields, with serde renames honoured
- [ ] Compile-time check that an audited handler takes the extractor
- [ ] Audit map read from the OpenAPI document at startup
- [ ] Migration: partitioned entries table with retention sub-partitions, GIN index; registered with partition runway
- [ ] Database model: write entry, write pending and confirmation, write refusal, query with filters, count by detail value over a window
- [ ] Audit layer and extractor on the private server
- [ ] Refusal entries from `TailscaleAdmin` and device auth
- [ ] Declare every private-server handler audited or silent, with recorded fields; the specs already naming audited actions (MNT, K8S, INV, UPG, RST, RPT) are audited
- [ ] Device API: audit issuance and record-changing handlers, silence status and reads
- [ ] Fleet query interface: declare each tool, add "Find audit entries"
- [ ] Recorder for jobs
- [ ] Log page in `private-web/`, with Playwright coverage
- [ ] `just gen-openapi`, and `just gen-api` if the public API's schema changes
