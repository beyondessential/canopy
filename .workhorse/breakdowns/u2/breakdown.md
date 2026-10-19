# Audit log follow-ups

The audit log itself is built on this card. These pieces build on it and go as cards of their own.

## Archive the audit log offsite · B5

Once a closed week's pending entries have all settled or lapsed, write it to the recovery escrow's storage as one Parquet object (empty weeks included), read it back, check it, and confirm it is under object lock before marking the week archived, with each object recording the hash of the previous week's. Check the previous week's object before writing each week, and each week the log page loads against the next, raising the self-alert for a broken chain. Once a week is archived, drop its ninety-day entries when they pass ninety days and the rest at a year. The archiver runs in the backups pod; the private server reads archived weeks back for the log page, only when an operator asks to load older entries, under a read-only role. Raise the self-alert for archiving falling behind. Needs both roles granted in the escrow bucket's account.

## Review the remaining surface for auditing · C5

This card audits only the features whose specs already name audited actions (maintenance windows, Kubernetes sleep and wake, inventory, upgrade plans, restore-replica declarations and issuance, reporting-schema declarations), the SQL playground, and the device API's issuance and record-changing handlers, and the applications and checks a status report creates; every other handler is declared silent. Go through the rest of the operator surface and the fleet query interface's queries, decide which actions are worth recording, and give each feature's spec the actions it records. Settle what the fleet query interface means by each query being attributable to its caller, and whether the recovery ceremony the backups spec calls audited records an action.

## Replace per-feature provenance with the audit log · D5

Drop the who columns that only record who did something (revoked by, resolved by, withdrawn by and the rest) and the backup credential issuance and schedule history tables, reading who did what from the audit log instead. Who columns that decide what happens next stay as the record's own state: a run lease's holder, which decides who is served the inventory, and a maintenance window's declarer and standing amender, which decide who may take a lease under it. Pages show who while the entry is in the database and when after that, so choose which actions are kept for a year where who matters for longer, such as revocations and withdrawn upgrade plans. The schedule editor shows its history from the log, and the recovery escrow's snapshot of schedule history reads from the log or drops it. Update each affected feature's spec to match, including the fleet query interface, whose incident and upgrade plan results name who resolved, recorded, amended and withdrew.
