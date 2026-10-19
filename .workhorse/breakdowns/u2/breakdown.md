# Audit log follow-ups

The audit log itself is built on this card. These pieces build on it and go as cards of their own.

## Archive the audit log offsite

Write each closed week of the audit log to the recovery escrow's storage as one Parquet object, read it back, check it, and confirm it is under object lock before marking the week archived, with each object recording the hash of the previous week's. Once a week is archived, drop its ninety-day entries when they pass ninety days and the rest at a year. The archiver runs in the backups pod; the private server reads archived weeks back for the log page under a read-only role. Raise the self-alert for archiving falling behind. Needs both roles granted in the escrow bucket's account.

## Replace per-feature provenance with the audit log

Drop the per-feature who columns (revoked by, held by, resolved by and the rest) and the backup credential issuance and schedule history tables, reading who did what from the audit log instead. Pages show who while the entry is in the database and when after that; the schedule editor shows its history from the log. The recovery escrow's snapshot of schedule history reads from the log or drops it. Update each affected feature's spec to match.
