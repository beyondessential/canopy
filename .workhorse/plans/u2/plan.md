# Audit log: plan

Implements [AUD](../../specs/platform/audit-log.md).
The reasoning behind these choices is in the working doc (`.workhorse/working-docs/u2/working-doc.md`).

This card builds the log itself: declarations, capture, storage, the log page, the fleet query tool, and the range self-alert.
Three pieces are split into their own cards (see the breakdown): the offsite archive, reviewing the rest of the surface for what to audit, and replacing per-feature provenance with the log.
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
- The extractor checks out a primary connection, begins a transaction, and hands the handler the connection and a recorder. The handler records `Action` values on it inside that transaction. The layer holds the same transaction and commits on a successful response, rolling back otherwise. Audited handlers that change something replace `state.db.get()` with it.
- On a successful response the layer checks that at least one action was recorded and that every recorded action is one the route declares. Otherwise it rolls back and returns an error, so a forgotten record fails the request and any test of the route.
- The recorder can commit an entry ahead of the handler's transaction, on its own connection, as pending. A later transaction settles it with `SELECT … FOR UPDATE` on the row: done in the handler's own transaction, with its other changes, or failed in a transaction of its own, since the handler's rolls back. Settling may merge outcome fields into the detail, such as an STS access key id. Outside effects, audited reads and A5's refusals all use this.
- An audited read commits its entry ahead, then reads as it otherwise would (`db_read`, or the playground's rolled-back transaction), and settles it done or failed. The layer settles it failed when the handler errors, which is safe for a read since it has no effect. For anything with an outside effect only the handler may settle it failed. The layer's at-least-one-action check counts the entry committed ahead.
- Device API handlers take the same extractor with the device as actor. Jobs record through the same recorder inside their own transaction with Canopy as actor.
- Cost, accepted: an audited request is one transaction held across everything its handler does, and a handler cannot commit part of its work before failing.

## Storage

- One table, partitioned by week through `database::partitions` and provisioned ahead with the other histories, so the HST runway alert covers it with no new code beyond registering the table.
- Each week is sub-partitioned by retention class, so the archive card's removals are partition drops: the ninety-day part once the week is archived and old enough, the rest at a year.
- Row: timestamp, actor kind and identifier, action name, the records acted on (each an identifier with its name as it was, and no foreign keys, since entries outlive and must never block deleting what they name), a `jsonb` detail, and for an entry committed ahead an outcome (pending, done, failed) and when it settled. The records acted on are indexed for the subject filter; a GIN index on the detail serves lookups and per-value counts.
- A trigger refuses every update except settling: outcome from pending to done or failed, once, within an hour of the entry being committed (by the database's clock), with detail only added to. Deletes are refused; entries leave by partition drop alone.
- Lapse replaces any freeze: the archiver reads a week only once its end is more than an hour past, so nothing in it can still be settled. Lapsed is derived (pending and over an hour old), not stored. The log page and "Find audit entries" leave out pending entries under an hour old.

## Reading

- The log page is a new route on the operator UI, readable by any administrator, filtering by actor, action, time within ninety days, and record acted on. It reads the database alone and offers older entries behind an explicit "load older" control. That control arrives with the archive card: each press fetches archived weeks newest first, filtering them in the private server, until it has a page of matches or has fetched a bounded number of weeks, then offers the next older week. Year-kept entries older than ninety days are read from the archive like the rest, so the page never mixes sources for one week.
- Every date and time on the page renders in the viewer's locale: `TimeAgo` for when, with its `toLocaleString()` tooltip, and `Intl.DateTimeFormat(undefined, …)` for times in an entry's detail and for the "load entries before" control. No fixed format strings; the mockup's dd/mm/yyyy is one locale's rendering, not the format.
- Actor icons follow the org-wide convention for action logs: humans in colour, everything else greyscale. An operator shows their profile picture, or an initial on a colour derived from their login where there is none (`UserAvatar` currently falls back to neutral grey). Devices, access tokens and Canopy itself get greyscale icons, Canopy's logo desaturated.
- The fleet query interface gains a "Find audit entries" tool, open to every caller of the interface, whose look-back is capped at ninety days whatever the retention, with the cap in its tool description and a longer window refused.

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
- [ ] Database model: write entry, commit an entry ahead, settle it, freeze a week, query with filters, count by detail value over a window
- [ ] Audit layer and extractor on the private server
- [ ] Audit the handlers of the specs already naming audited actions (MNT, K8S, INV, UPG, RST, RPT) and the SQL playground; declare every other private-server handler silent, for the follow-up review card to revisit
- [ ] Device API: audit issuance and record-changing handlers, silence status and reads; a status push records each application arrival and each check registration
- [ ] Fleet query interface: declare each tool silent, add "Find audit entries"
- [ ] Recorder for jobs
- [ ] Log page in `private-web/`, with Playwright coverage
- [ ] `just gen-openapi`, and `just gen-api` if the public API's schema changes
