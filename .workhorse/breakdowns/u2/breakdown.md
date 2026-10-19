# Audit log follow-ups

The audit log itself is built on this card. These pieces build on it and go as cards of their own.

## Archive the audit log offsite

Freeze each closed week of the audit log against settling, then write it to the recovery escrow's storage as one Parquet object, read it back, check it, and confirm it is under object lock before marking the week archived, with each object recording the hash of the previous week's. Once a week is archived, drop its ninety-day entries when they pass ninety days and the rest at a year. The archiver runs in the backups pod; the private server reads archived weeks back for the log page, only when an operator asks to load older entries, under a read-only role. Raise the self-alert for archiving falling behind. Needs both roles granted in the escrow bucket's account.

## Replace per-feature provenance with the audit log

Drop the who columns that only record who did something (revoked by, resolved by, withdrawn by and the rest) and the backup credential issuance and schedule history tables, reading who did what from the audit log instead. Who columns that decide what happens next stay as the record's own state: a run lease's holder, which decides who is served the inventory, and a maintenance window's declarer and standing amender, which decide who may take a lease under it. Pages show who while the entry is in the database and when after that, so choose which actions are kept for a year where who matters for longer, such as revocations and withdrawn upgrade plans. The schedule editor shows its history from the log, and the recovery escrow's snapshot of schedule history reads from the log or drops it. Update each affected feature's spec to match, including the fleet query interface, whose incident and upgrade plan results name who resolved, recorded, amended and withdrew.
