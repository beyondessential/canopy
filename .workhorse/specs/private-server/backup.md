---
id: BKO
---

# Operator backup control

An operator configures, through Canopy, how a server group backs up: where its repo lives, on what cadence, with what retention, and which machines and types participate.
Canopy owns the repo passphrase throughout — it is generated or accepted once, stored in Canopy's secret store, and never handed back except through the audited recovery ceremony (see [ESC](../jobs/escrow.md)).

## Scope

This spec covers the operator-facing control surface: per-group backup configuration and its lifecycle, scheduling and retention, per-machine participation, on-demand backups, and the status view.

It does not cover the device contract (see [BAK](../public-server/backup.md)) or Canopy's autonomous maintenance, inspection, detection, and alerting (see [BKJ](../jobs/backup.md)).

Reads are available to any tailnet user; changes require an administrator.

## Per-group configuration

A group has at most one backup configuration: the bucket, prefix, region, the cross-account roles Canopy assumes, the reference to the group's passphrase, and its placement and lifecycle state.

Placement is one of:

- **external** — the operator brings their own bucket and supplies the role ARNs Canopy will assume.
- **shared** — Canopy provisions and names a bucket in its own shared account; the operator supplies nothing about location.

A configuration is created once and its structural fields (bucket, roles, placement) are immutable; the region and the operational settings below are editable.
Decommissioning a group deletes its configuration row — which stops all credential issuance for the group — and deletes the Canopy-owned passphrase.
The bucket and its object-locked contents persist independently and are not Canopy's to delete; teardown is a separate, deliberate act gated by the lock window.

## Lifecycle and provisioning

A configuration moves from **provisioning** to **ready**; devices are refused until it is ready.
Creating a configuration sets it provisioning and asks Canopy to create or connect the repo; that work transitions the configuration to ready, or records the error it failed with so the operator sees why.
The operator interface depends only on these observable states, not on how provisioning is carried out.

A configuration may also be created or reconciled idempotently by machine — for infrastructure-as-code — under administrator-equivalent authentication, with the same probe and provisioning behaviour as the interactive path.

## Setup and the passphrase

When a configuration is created, Canopy probes the target bucket and classifies it: empty, an existing kopia repo, holding unrelated content, or inaccessible.
The classification chooses the mode:

- **from-birth** — an empty bucket; Canopy generates a fresh passphrase and creates a new repo.
- **passphrase** — an existing repo; the operator supplies its passphrase and Canopy connects to it. On adoption Canopy disables any repo-level object-lock retention the existing repo carries: immutability is the bucket's Object Lock and expiry is Canopy's maintenance, and a live repo-level retention mode would block both device writes and maintenance reclamation.

A bucket holding unrelated content is refused rather than written into; Canopy never deletes to make room.
Either way Canopy creates and owns the passphrase secret, and configuration and secret are created together — if the secret cannot be stored, the configuration is rolled back, so a configuration never exists without its passphrase.
The supplied or generated passphrase is only the starting point: Canopy rotates it on a cadence thereafter (see [BKJ](../jobs/backup.md)), and the recovery ceremony recovers whatever the current passphrase is.

## Scheduling and retention

Each `(group, type)` has an expected backup interval and a retention policy, taken from a per-`(group, type)` override when set, otherwise from the fleet-wide default for that type.
A manual-only type has no interval and is backed up only on an explicit request.
Retention is floored to an organisational minimum; a configuration may deliberately opt out of the floor, which is recorded as the dangerous choice it is.

## Participation and on-demand

A machine participates in a type when that type is an enabled capability on it; an operator toggles participation per `(machine, type)`.
Participation is a machine's, not an application's: a box hosting two workloads is one participant, and an operator configures it once (see [FLT](../servers/overview.md)).
An operator may queue a one-off backup — or restore — for a `(machine, type)` to run on the next cycle, and may cancel a queued one before it runs.
An operator may also request a one-off full maintenance run for a group, to reclaim storage or apply repo-settings changes without waiting for the scheduled cadence, and may cancel it before the scheduler picks it up (see [BKJ](../jobs/backup.md)); at most one such request is pending per group.

### Allowing a restore

An ad-hoc restore reads the group's whole backup history, so it is refused unless an operator has deliberately allowed it for the machine being restored.
An operator opens the machine's restore window; it stays open for a day and then closes on its own, and opening it again restarts that day.
An operator may close it early, and closing an already-closed window is accepted without effect.

The window is opened over a machine, since a restore rewrites the box rather than one workload on it.
Canopy records who opened it and shows the operator when it closes, both on the group's backup view and on the machine's own page.
An expired window reads as closed everywhere it is shown, so the figure an operator reads is what a device would be granted.

## Status

The operator can see, per group: the repo's size and cost basis, recent runs with their outcomes and errors, recent maintenance, the latest snapshot per machine, and any in-flight or pending one-off requests — including a pending on-demand full maintenance request and who made it.

Each run's duration is the time from its first credential issuance to its report; a run that re-issues credentials while it runs is measured from the first issuance of the sequence.
A run for which credentials were issued but no report has arrived is shown from that issuance: in progress while the credentials remain valid, otherwise as a run whose outcome is unknown. This makes runs that don't report — such as an ad-hoc restore — visible; a later report for the same run replaces its issuance-derived entry.
Where a run reported the moment its data was frozen (see [BAK](../public-server/backup.md)), that moment is shown alongside its report time, so a backup whose data is materially older than its upload is not read as a fresh one.

For a run reporting progress, the operator can see how far it has got without waiting for it to finish: what it has transferred against the total it expects, its current transfer rate, and how long since the device last made contact.
These figures advance while the view stays open, without the operator reloading it; the view watches more closely while a run is in flight than when it is showing settled history.
They appear wherever a backup of a type is shown as running — on the group's activity and per-machine views, and on the machine's own page — so an operator watching a particular machine need not go to the group to see whether its backup is moving.
A run whose device reports no progress is still shown as in progress, unchanged from a run that cannot report it — an absent figure reads as unknown rather than as zero or as a fault.
The rate over a run's life is available as a series, for a run in flight or a finished one, for as long as its progress is retained.
The backup engine's own transferred figure and the object-storage traffic Canopy's proxy tallied are shown against each other, so a divergence between what a run believes it sent and what crossed the wire is visible.
Engine-specific detail a device reported that Canopy does not model is available verbatim for inspection.
