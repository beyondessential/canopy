---
id: BKJ
---

# Backup control plane

Canopy maintains, verifies, and watches over the fleet's backups itself — clients neither run maintenance nor hold the rights to.
This is the autonomous half of the backup system: the work Canopy does on a cadence with no device asking, and the health signals it raises from it.

## Scope

This spec covers Canopy's own background backup work: repo maintenance, inspection, storage metering, upstream preflight, housekeeping of the data devices report, and the detection and alerting that turn all of it into incidents.

It does not cover the device contract (see [BAK](../public-server/backup.md)), the operator's configuration of a group (see [BKO](../private-server/backup.md)), or restore-health (the managed restore replicas spec, `RST`).

Canopy acts only on groups whose configuration is ready, runs at most one operation per group at a time, and bounds how many groups it works on at once.

## Maintenance

Canopy runs each group's repo maintenance on a cadence — clients are never granted the rights to.
It enforces the group's retention as part of maintenance, and records every run's outcome so a stuck or failing maintenance is itself detectable.
Maintenance also re-asserts that the repo carries no repo-level object-lock retention mode of its own, healing a repo that was imported with one before Canopy disabled it (see [BKO](../private-server/backup.md)).
Beyond the cadence, an operator may request a one-off full maintenance run for a group (see [BKO](../private-server/backup.md)); Canopy runs it on the next scheduling opportunity, ahead of the jittered cadence slot, subject to the same one-run-per-group interlock — so a forced run never overlaps an in-flight one.

## Passphrase rotation

Canopy rotates each group's repo passphrase on a cadence, so a leaked passphrase is useful only until the next rotation rather than indefinitely.
Rotation is crash-safe: an interrupted rotation is reconciled on the next attempt, and throughout it the repo stays openable with either the previous or the new passphrase — it is never left unopenable.
Rotation contends with the rest of a group's background work for the one-operation-per-group interlock; losing it defers the rotation to the next opportunity within the same period rather than to the next period, so the cadence holds even when a group is persistently busy at its scheduled moment.
Like maintenance, rotation is Canopy's to do; operators never run it.

## Recovery escrow

Because Canopy holds the only copy of every passphrase, it escrows the state needed to recover access without it (see [ESC](escrow.md)): the per-group passphrases and repo coordinates, and the group, machine, configuration, schedule, and capability records that frame them.

## Inspection

Canopy periodically inspects each group's repo against the storage directly, independent of what devices reported:

- it verifies repo integrity, and a failed verification is repo corruption;
- it inventories the repo as the ground truth a device's report is reconciled against: the latest snapshot per source, and the identity of every snapshot it found, so a run's reported snapshot can be looked up rather than inferred from timestamps.
  The inventory describes the repository as it stands at that inspection, so a snapshot expired by retention stops being recorded;
- it records repo size, logical and physical, and the storage cost basis for display;
- it records each snapshot's logical size and matches it to the device run that produced it by snapshot id, so the repo's own size stands in when a run reported none, and is cross-checked against the size a run did report. A snapshot's recorded size is written once, because a snapshot is immutable.

## Upstream preflight

Canopy watches its own access to each group's storage, so a broken control plane is caught at the source rather than when the fleet starts failing.
It checks that its identity resolves, that it can assume each group's role and perform a read-only no-op, and that the bucket's object-lock is present and at least the required retention.
Preflight only alerts; it never pulls Canopy out of service, because a failing check must not make a degraded situation worse.

## Housekeeping

Canopy prunes the in-flight progress series devices report (see [BAK](../public-server/backup.md)) once it ages past the period that series is retained for, so it stays bounded without operator involvement.
Pruning is fleet-wide and independent of any group's maintenance, so it never waits on or delays a group's other backup work.

## Detection

Canopy reconciles three sources — what a device reported, what credentials were issued, and what actually landed in the repo — and surfaces where they disagree:

- **staleness** — a machine with a prior successful backup but none recent, or one that has never backed up though it has been expected long enough. Expectation for one that has never backed up starts from the later of its group's backup configuration and when the machine was enrolled, so a machine onboarded into an existing configuration is not stale the moment it appears. Recency is the age of the *data*, measured from the moment the backup froze what it captured (see [BAK](../public-server/backup.md)), falling back to the run's report time when it reported no such moment. So a backup that took many hours to upload is aged from when it was taken, and a machine whose data is a day old is not counted fresh because its upload finished minutes ago. Both which run counts as the latest success and how old that success is use the same measure, so a machine's freshness never travels backwards as new runs arrive.
- **reconcile** — a device reported a successful backup naming the snapshot it created and the repository does not hold that snapshot (the report is false or the upload didn't persist), or a fresh snapshot exists but no recent report (the reporting path is broken).
  These assert that the reporting path itself is working rather than anything about the age of the data.
  A snapshot's absence is only evidence once someone has looked: no verdict is reached from an inventory older than the run it would contradict, and a run whose snapshot could have been expired by retention since it was reported is not judged at all.
  Where a verdict cannot be reached the signal is neither passing nor failing, and one that cannot be reached for any of a machine's backup types leaves whatever was already raised standing rather than clearing it.
- **snapshot recency** — the newest snapshot the repository holds for a machine's source is older than the moment the device's latest run says it froze its data.
  Backups and repository inspection run on independent cadences, so this compares two observations that are routinely out of step with nothing wrong; it is recorded and presented for context but never alerts, and it is only reached where the run reported the moment its data was frozen and the source has been inspected since.
- **size** — a device reported a snapshot size that disagrees with the size the same snapshot occupies in the repo; only compared when both sizes are known and non-zero.
- **maintenance** — a group whose maintenance is overdue, or whose most recent maintenance failed.

## Alerting

**No backup signal is a failure by default.**
A failure in Canopy means a live service is down, and it is acted on within minutes; a backup that is late, unreconciled, or unverified is not that.
The fleet's backups are layered, so a single missed run — a six-hourly backup that slips one cycle and succeeds on the next — is something to look at, not something to wake anyone for.
Backup signals therefore default to a warning and do not escalate, and an operator may raise an individual one to a failure through its policy in consultation with the people who answer the resulting alerts.

Three signals are the exception, and default to an escalating failure: repo corruption, a rotation that left the repository openable by neither passphrase, and object-lock protection that is missing or weakened.
Each of these means the backups are already gone, unrecoverable, or unprotected, rather than merely late.

A signal whose evidence supports only that something looks off, rather than that anything is wrong, ranks below a warning: it is recorded and presented with what it observed, and never alerts.
Snapshot recency is one, and its wording says what was observed rather than what it implies, so an operator reading it is not told a backup is missing on the strength of two timestamps.

Backup alerts are raised at one of two scopes:

- **Per-machine** signals (staleness, never-backed-up, the report-gap, the size discrepancy, the missing-snapshot reconciliation, and restore-verification — see the managed restore replicas spec, `RST`) are subject to the machine's monitoring gate: still recorded for visibility, but they contribute to an incident only when the machine is monitored, because some machines are intentionally intermittent.
- **Group-level** signals (repo corruption, maintenance failure, preflight failures) page regardless of any member's monitoring state, because they are control-plane concerns that belong to no single machine.

A signal about one machine is raised against that machine, even when the condition it detects is a disagreement between the agent's report and the group's repository.
Two machines in a group failing the same check hold two separate alerts, and one machine's recovery never clears another's.

Each signal is one check, whatever the machine backs up.
A machine backing up its database, its configuration, and its reverse-proxy configuration has one staleness signal, not one per backup type — the types it is stale for are carried in the signal's detail and named in its text, and the signal is graded per type before it settles on the most urgent of them, as any check with instances is (see [CHK](../monitoring/checks.md)).
So an operator configures staleness once for the fleet and, where a particular type warrants different treatment, writes a rule for that type rather than acquiring another check to configure.
The same holds for the restore signals, which have both a backup type and a restore intent.

Each signal has a stable key by which operators silence or snooze it and by which the interface and notifications refer to it; the keys are a contract and are not renamed without migrating stored silences.
Where an alert's text names the machine it concerns, it names it the way an operator knows it, falling back to an identifier only when the machine has no name.
A signal recovers when the condition that raised it clears.
