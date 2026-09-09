---
id: RST
---

# Managed restore replicas

Canopy is the control plane for a fleet's *managed restore replicas*: standing replicas that Canopy decides should exist and keeps restored from the latest backups, driven through a restore consumer.
An external restore consumer — first-party infrastructure that restores backups into working Postgres replicas — is driven entirely by Canopy: Canopy declares which replicas should exist, hands out the snapshot to restore and short-lived read-only credentials for each, and records the restorability of every replica as the strongest backup-health signal.
A replica restored from real data is also the substrate for testing a Tamanu version's schema migrations before that version reaches the group, so the same machinery carries both a backup-health signal and a version-readiness one.

## Scope

This spec covers *managed* restore replicas only: the standing replicas Canopy decides should exist and keeps current, the restore-health signal they produce, and the pre-upgrade migration testing they carry.

It does not cover an operator restoring a backup by hand.
An operator performing disaster recovery or an ad-hoc restore selects a specific snapshot for a specific machine and restores it through that machine's own device tooling and credentials — the operator-driven per-machine restore path.
That path is operator-driven and machine-scoped: the operator chooses what to restore and where, and Canopy only issues the read-only credentials and snapshot information for that one machine.
Managed replicas are the opposite mode: Canopy chooses what should be restored, continuously, with no operator selecting each one.
The two modes share Canopy's read-only credential issuance and snapshot authority; they differ in who decides what gets restored.

## Why it exists

A backup is only as good as its last successful restore.
Producing snapshots (a device backed up) and confirming they landed in the repo (a snapshot exists) are weaker guarantees than actually restoring one into a live database.
Canopy already knows every group, every machine, every backup type, and the latest snapshot for each — so it is the natural authority on *what should be restored*.
Centralising that decision in Canopy eliminates the long-lived AWS keys a restore consumer would otherwise hold, makes the restore consumer a stateless executor of Canopy's intent, and closes the lifecycle loop end-to-end: produced, persisted, restorable.

## Actors

A **restore consumer** is first-party infrastructure that restores backups and reports their health.
It holds no standing access to any backup repo and stores no list of what to restore: it asks Canopy what replicas should exist, restores them, and reports back.
It owns only the mechanics of restoration — how a replica is provisioned, where it runs, how much storage it gets, when it is torn down.

An **operator** declares, through Canopy, which replicas should exist and why.

Canopy owns the *what* and the *why* (which group, which machine, which type, to what end, how fresh) and the *authority* (which snapshot, which credentials, is it restorable).
The consumer owns the *how*.
This boundary is load-bearing: Canopy never models a consumer's runtime placement, and a consumer never decides on its own what to restore.

## Identity and authorization

A restore consumer authenticates as a single device holding the `backup-restore` role.
The role is generic: any future restore consumer uses the same role with its own declared replicas.
A `backup-restore` device has no implicit machine and no implicit group; it is not a member of any group it reads.

The role is read-only by contract, enforced at the API:

- A `backup-restore` caller requesting backup (write) credentials is rejected.
  The read-only guarantee is enforced by Canopy rather than trusted to the caller, so a compromised consumer cannot pivot to writing or poisoning a repo.
- A `backup-restore` caller may obtain credentials and the worklist only for a `(group, type)` it has been authorised for.

Authorization is the set of declared replicas (below): a consumer is authorised for exactly the `(group, type)` pairs that appear in its enabled replica declarations.
There is no separate grant object — declaring a replica *is* the authorization to read what that replica needs.

A device reaches this role through one-off operator promotion, the same path a release-publishing device uses; no fleet-enrolment flow is involved.
Either transport Canopy already accepts for devices — tailnet identity or a client certificate — satisfies the role; the role, not the transport, is the contract.

## Consumer capabilities

A restore consumer advertises the set of intents it can satisfy, and registers it with Canopy when it starts and whenever it changes.
Each advertised intent carries:

- a stable **name**, an open identifier for the intent;
- a human-readable **description**, shown to operators choosing an intent;
- a set of **semantics**: well-known tags, defined by Canopy, that the consumer opts the intent into, each granting Canopy a specific behaviour for that intent (see [Semantics](#semantics));
- a **parameter schema**: named, typed settings the consumer accepts per replica of the intent, each optionally with a default (see [Parameters](#parameters)).

Canopy stores the advertised set against the consumer and treats it as the authority on what that consumer can be asked to do and how.
Registration replaces the consumer's whole advertised set.

The advertised set governs three things:

- **What can be declared.** Canopy offers operators the intents the chosen consumer advertises when they declare a replica, along with each intent's description and parameter fields.
- **What is dispatched.** A consumer's worklist includes only entries whose intent it currently advertises; Canopy never asks a consumer to satisfy an intent it has not advertised.
- **How Canopy behaves.** The intent's semantics select Canopy's dispatch and alerting behaviour for its replicas.

When a consumer's set grows, the new intents become available for operators to assign, so a consumer gaining a capability is reflected without operator guesswork.
When a consumer's set shrinks, any enabled declaration whose intent is no longer advertised becomes a *gap*: Canopy drops it from the worklist immediately and surfaces it to operators as a declaration no consumer can currently satisfy, to reassign or retire.
A gap is a configuration state shown to the operator, not a restore-health incident; the backups themselves are unaffected.

Intent is an open set defined by consumers; Canopy fixes no canonical list.
For example, an intent that restores a snapshot solely to prove it is restorable and then discards it opts into `check` and `once`; an intent that keeps a queryable replica running opts into `check` and `url` and accepts parameters such as a minimum uptime and a size cap.

### Semantics

A semantic is a Canopy-defined behaviour an intent opts into.
Canopy acts only on the semantics it recognises; an unrecognised semantic is stored and preserved but changes no behaviour, so a consumer may advertise ahead of Canopy support.
The recognised semantics are:

- **check** — the intent produces restore-health feedback.
  Canopy expects a report for each of the intent's replicas and holds it to an overdue bound (see [Alerting](#alerting)).
  An intent without `check` is dispatched but never reported on nor alerted.
- **once** — a given snapshot is dispatched to the intent at most once.
  Canopy omits a replica from the worklist once the intent has a healthy report for that replica's current snapshot, and reinstates it only when a newer snapshot exists.
  Without `once`, the intent is always pointed at the latest snapshot and manages its own refresh.
  An intent whose result depends on more than the snapshot keys `once` to that wider input as well, and may treat a failure as settled rather than retryable (see [Pre-upgrade migration testing](#pre-upgrade-migration-testing)).
- **url** — the intent's health report carries a link to the running replica within its attached health data, which Canopy surfaces to operators.
- **migrate** — the intent applies a Tamanu version's schema migrations to the replica it restores.
  Canopy names a target version on each of the intent's worklist entries and withholds an entry where no application on the machine has a candidate version.
  `once` for such an intent is keyed to the snapshot and the target version together (see [Pre-upgrade migration testing](#pre-upgrade-migration-testing)).
- **redact** — the intent can de-identify the restored data before serving it.
  Canopy offers redaction as an option on each of the intent's replicas, supplies the masking manifest for the product being restored, and holds a redacting replica to the outcome of its redaction (see [Redaction](#redaction)).

### Parameters

An intent's parameter schema names the settings the consumer accepts for each replica of that intent.
Each parameter has a type — a duration, a size in bytes, a boolean, an integer, or text — and may carry a default.
Canopy uses the schema to collect values when a replica is declared, validates the values against it, stores them with the declaration, and returns them in the worklist so the consumer receives its per-replica settings.
Canopy does not interpret parameter values beyond their type; they are settings passed through to the consumer.
The exception is a parameter Canopy owns on behalf of a semantic it recognises: Canopy resolves such a parameter's value itself, the operator does not set it, and a value stored against the declaration for it is preserved but not sent while Canopy owns it (see [Redaction](#redaction)).

Every parameter is optional: an operator may leave any one unset.
The worklist carries a resolved value for each parameter the intent advertises, and only for those: an unset parameter that has a default is sent as its default, and an unset parameter without a default is sent as JSON `null`.
A value for a parameter the intent no longer advertises is preserved with the declaration rather than rejected, mirroring how an unrecognised intent is preserved, but is not sent in the worklist.

## Declared replicas

An operator declares replicas against Canopy.
Each declaration carries:

- the **group** whose repo holds the backups;
- the **type** of backup to restore;
- a **machine** within the group, or all machines in the group when none is named;
- an **intent** the chosen consumer advertises;
- a human-readable **name**, distinct from every other declaration assigned to the same consumer;
- **parameter values** for the intent's schema, defaulted where the schema provides one;
- whether the replica **redacts**, offered only for an intent carrying `redact` (see [Redaction](#redaction));
- an **overdue bound**: the maximum time a replica may go without meeting its intent's health expectation before Canopy considers it overdue, interpreted per the intent's semantics (see [Alerting](#alerting));
- whether the declaration is **enabled**.

A declaration's intent must be one the chosen consumer advertises (see [Consumer capabilities](#consumer-capabilities)); a declaration whose intent is unadvertised is a gap, surfaced to the operator and never dispatched.

The name identifies the replica to the consumer that maintains it and to the operator reading the list, so a consumer's declarations must not share one.
It is the only thing a declaration is unique on: an operator may declare as many replicas of one group, type, intent, and machine as they have uses for — a raw one alongside a redacted one, a nightly one alongside a weekly one — and Canopy tells them apart by name alone.
A declaration that reuses a name already assigned to that consumer is refused rather than accepted as an ambiguous pair.

The name is part of a replica's identity rather than a label on it, so each named replica is dispatched, reported on, and alerted on separately.
A whole-group declaration and a machine-specific one that both cover a machine are two of that machine's replicas, and each gets its own worklist entry.
A report identifies the declaration it came from, and Canopy records that declaration's name alongside the group, machine, type, and intent it already keeps, so the report goes on naming its replica once the declaration is retired.
Naming a declaration is required of every report, because several replicas can share a group, machine, type, and intent, and a report that named none could not be attributed to one of them.
The declaration must still exist and belong to the reporting consumer; a report naming a retired declaration is refused, since a replica nothing declares any more is not one Canopy tracks and a finding recorded against it could never recover.

A declaration scoped to a whole group expands to one replica per current machine in that group.
Machines joining or leaving a group change what the consumer is asked to maintain, with no per-machine operator action.

Declarations are managed through the operator interface (create, edit, enable/disable, delete) and are audited.
Deleting a declaration stops the consumer being asked to maintain that replica and revokes its authorization for that `(group, type)` if no other declaration covers it; recorded restore-health history is retained.
The reports it collected survive the deletion, keeping the group, machine, type, and intent they concern but no longer naming a declaration, so a declaration that has been reported on is no harder to retire than one that never was.

## The worklist

A restore consumer fetches its complete desired state from Canopy in one request, scoped to the calling consumer.
Canopy expands the consumer's enabled declarations — those whose intent the consumer currently advertises — against the current machines and the latest known snapshot for each, and returns one entry per concrete replica the consumer should currently act on:

- the declaration's identifier, group, machine, type, intent, name, and overdue bound;
- the resolved **parameter values** for the replica, one per parameter the intent advertises (unset parameters resolved to their default, or JSON `null` when they have none);
- the **snapshot to restore**: the snapshot identifier and its timestamp, or empty when no successful backup is yet known for that machine and type;
- the repo coordinates needed to locate the backups (storage, bucket, prefix, region).

The worklist does not carry credentials or the repo password.
The consumer reconciles the worklist against what it is actually running — creating, refreshing, and tearing down replicas to match — and is responsible for converging on the desired state over time.

An intent carrying `once` contributes an entry only while the latest snapshot for its `(machine, type)` has no healthy report; once that snapshot is verified the entry is omitted until a newer snapshot exists.
An intent without `once` always contributes an entry naming the latest snapshot.

### Latest state, not a queue

Each entry names the *latest* snapshot for its `(machine, type)`, not a backlog to drain.
A consumer restores on its own cadence and skips the intermediate snapshots produced since its last restore; restoring less often than backups are produced is expected, not a failure.
A restore can take far longer than the interval between backups — the data is slow to download and restore, and a persistent replica may be held up while its workload runs.
A `once` intent verifies each snapshot at most once and is not asked to restore again until a newer snapshot exists, so its work follows the backup cadence rather than the clock; an intent without `once` is always pointed at the latest snapshot and refreshes on its own cadence.

### Snapshot authority

The snapshot Canopy hands out for a `(machine, type)` is the snapshot identifier of that machine's most recent successful backup run of that type.
This is the same snapshot the operator interface shows as the machine's latest.
Canopy's independent repo inventory corroborates the snapshot's existence and timestamp; it is not currently the source of the identifier.

## Credentials

A consumer obtains credentials per `(group, type)` as it works, not for the whole fleet at once.
Canopy verifies the caller has an enabled declaration covering that `(group, type)`, then issues:

- short-lived read-only object-storage credentials scoped to the group's repo;
- the repo password.

The credentials permit reading the repo and nothing else; they cannot write, overwrite, or delete.
Each issuance is audited.
A consumer may include an optional run correlation identifier with a credential request; Canopy records it on the issuance so the run is tied to its later health report.
Absence of a covering declaration is a definitive refusal, not a transient error, and a consumer surfaces it as a clear failure for the operator to diagnose by inspecting the declaration in Canopy.

The 1-hour lifetime of an issued credential does not bound restore duration: a consumer refreshes credentials as needed across a long restore.

## Restore-health reporting

A consumer reports the outcome of each replica back to Canopy.
A report carries:

- the declaration, group, machine, and type it concerns;
- the **snapshot** that was restored, joining the report to the produced-and-persisted record for that snapshot;
- the **outcome** — restored-and-healthy, or failed — and, on failure, an error description;
- whether the restored database came up healthy, and its Postgres major version;
- when the restore was observed;
- the object-storage traffic the restore moved;
- optionally, the same run correlation identifier used when the credentials were obtained, tying the report to its issuance;
- optionally, arbitrary health data the consumer chooses to attach (e.g. cluster statistics, whether indexes needed fixing). Canopy stores and displays it as-is without interpreting it; specific fields may later be promoted to first-class, queryable form.

When the intent carries the `url` semantic, the attached health data includes a link to the running replica, which Canopy surfaces to operators alongside the report.

Restored-and-healthy means the snapshot restored, the database started, and the consumer's readiness checks passed — a stronger statement than a snapshot merely existing.
A failure covers any stage: the restore itself, the database failing to come up, or a readiness check failing.

Only intents carrying `check` are expected to report; an intent without it is dispatched but produces no restore-health signal and is never alerted on.

Reports are retained indefinitely as an audit trail.

Canopy derives each restore's duration from the interval between its first credential issuance and its report.
A restore for which credentials were issued but no report has arrived is shown as in progress while the credentials remain valid, otherwise as a restore whose outcome is unknown; this surfaces in-flight and terminated-without-report restores in the operator view, including those under intents that produce no health report.
The derivation covers consumers only: a device belonging to one of the group's machines takes restore credentials for its own purposes (a clone refresh, an operator's manual restore) and never reports, so its issuances derive no restore activity.

## Pre-upgrade migration testing

An intent carrying the `migrate` semantic applies a Tamanu version's schema migrations to the replica it restores, so a version's effect on a group's real data is known ahead of an upgrade window rather than discovered inside one.

An upgrade applies schema migrations to the live database with the application down for the duration.
A migration that fails against real data, and one that succeeds but runs far longer than the window allowed for, are both properties of that group's data rather than of the migration alone, so neither shows against a small or synthetic database.
Canopy knows the version each application reports running and the upgrade path it would be served, and already holds the authority over replicas made from real backups, so it is where the question can be posed ahead of the window.

### Candidate versions

A candidate is a property of an application, because a version is what an application runs (see [FIG](../private-server/figures.md)).
The data the candidate is tested against is a property of a machine, because a snapshot is what a machine backed up.
A migration test therefore restores the snapshot of the machine an application runs on and applies that application's candidate version's migrations to it.

Canopy decides which versions are tested against which applications, rather than an operator naming each pair.

An application's candidate is the version its own environment's open plan moves it to: the plan for its group at its rank (see [UPG](../private-server/upgrade-plans.md)).
An environment with no open plan has no candidate, so none of its applications are tested, and an application with no rank follows its group's headline environment (see [GRP](../servers/groups.md), "Environments").
A site's clone is often planned ahead of its production, so the two are tested against different versions at once.

Recording a plan is what asks for the testing.
A run costs hours of a consumer's capacity per replica, and which minor an environment moves to is not something Canopy can derive, so aiming at whatever is newest would spend that capacity on versions nobody has decided to take.
An environment that wants its data tested says where it is going, and gets an answer about the version it will actually apply.

One candidate, not one per version along the path.
Migrations are applied to the restored snapshot in sequence, so a run targeting the planned version applies every migration between the snapshot's version and that one, and exercises the whole chain an upgrade would.
Where a chain does break, the failing migration named in the report identifies the step without a second run.

Only a published version is a candidate, because a version's migrations reach a consumer as its published artefacts, and an unpublished version has none to fetch.
Publication is what makes a version testable and what makes it reachable by an application, so the two arrive together.

Only an application running Tamanu has candidates, because the migrations under test are Tamanu's and no other type has an upgrade path through them.
An application whose machine has no successful backup of a restorable type has no candidates either, because there is nothing to restore and migrate.

Testing therefore sits between a version being available and a group being told to take it.
That window is where the answer is still cheap: the fleet is not moving yet, and a version found to break a group's data can be held back before anyone schedules its upgrade.

### Dispatching a migration test

`migrate` is a semantic an intent opts into, and an intent carrying it carries no other purpose.
It carries `check` alongside, so a single restore reports the replica's health and the migrations' outcome as two signals from one report.

An intent carrying `migrate` is withheld where no application has a candidate version.
An intent that verifies backups therefore does not also migrate: it would go undispatched for every machine without a candidate, leaving the backups of any non-Tamanu application, and of every environment with no plan open, unverified.
An intent that keeps a replica queryable does not migrate either: a migrated replica sits at a version its group is not running, so a declaration promoted to it would give an operator a schema that does not match production.

A verifying intent and a migrating intent restore the same snapshot separately.
A verifying intent restores once per snapshot, and a migrating intent's `once` is keyed to the snapshot and target version together, so it restores when a new candidate version appears rather than on every snapshot.

An entry for a `migrate` intent names the target version alongside the snapshot, and the application whose candidate it is.
A report echoes that application back, so the finding lands on the workload the version belongs to rather than being re-derived; a consumer that does not send it has it resolved from the machine and the version.
A consumer obtains that version's migrations from its published artefacts, the same way an application being upgraded does, so naming the version is the whole reference it needs.

A machine none of whose applications has a candidate version contributes no entry, whatever its declaration says.
There is nothing to migrate to, and an entry naming no version would ask a consumer to restore a database for no reason.

`once` is keyed to the pair of snapshot and target version: an entry is omitted once that pair has a verdict, and reinstated when either a newer snapshot or a new candidate version appears.
A failed verdict settles that pair rather than leaving it retryable.
A restore can fail for transient reasons and is worth retrying, but a migration failing against a fixed snapshot fails the same way every time, and a retry costs a full restore for an answer already held.

### What a migration test reports

Beyond the fields every report carries, a migration-testing report carries:

- the **target version** whose migrations were applied;
- whether **every migration applied**, or which one failed and the error it produced;
- the **total elapsed time** of the migration run;
- the **elapsed time of each migration** that ran;
- the **size of the data**, both before the migrations ran and after.

The report's outcome and replica health describe the restore, and the named failing migration describes the migrations.
A restore that succeeded into a healthy replica whose migrations then failed reports a healthy restore and names the migration that failed.
Keeping the two apart is what lets restore health and version readiness stay separate signals: the backup restored, so the backup is fine, and the finding belongs to the version.

Per-migration timings are a primary result rather than diagnostic detail.
A version whose migrations all apply but whose slowest migration takes hours against a large database is a finding, and a report carries enough detail to name the migration to attend to.

The size before the run is what lets a duration be read against the volume that produced it, and compared across databases of different sizes.
The growth between the two figures is its own finding: a migration that backfills a large table leaves the machine needing disk it was not provisioned for, and every group that has yet to run it will grow the same way in proportion to its own data.
Growth is also a second reason a window overruns, since the write volume that produces it is time the application spends down.

### Verdicts

Canopy derives a verdict for each candidate pair of application and version: not yet tested, passed, or failed.
Verdicts are presented per group, as the set of versions tested against that group's applications, so whether a version is safe for a group is answered in one place instead of by assembling reports.

A verdict names the snapshot it was reached against and when that was, because a pass against a month-old snapshot is a weaker statement than one against last night's.
A newer test of the same pair supersedes the previous verdict, and the superseded reports remain.

Whether an attempt is under way is carried beside the verdict rather than folded into it.
A restore takes hours, so a group with a test running would otherwise read as untested for the whole window, and a consumer that has quietly stopped would look the same as one that has not started.
An attempt is in flight while credentials have been issued and not yet expired with no report, and is a run that ended without reporting once they have, exactly as [Restore-health reporting](#restore-health-reporting) already derives for restores.

Keeping the two apart is what lets a pair read as failed with a fresh attempt already running.
Folding the attempt into the verdict would overwrite the answer with the activity and lose the finding.

### Version readiness

A failed migration test marks its target version as carrying a known issue, which removes that version from those considered ready to roll out, and records the application and the failing migration so whoever picks it up knows whose data provoked it.
This is the gate an operator-filed known issue uses, so a failure found automatically and one found by hand have the same effect on a rollout.
Clearing the issue is an operator action, and a later passing test does not clear it, because whether the resolution is a change to the migration, a change to the data, or an accepted limitation is a judgement.

## Redaction

An intent carrying the `redact` semantic can serve a replica with its data de-identified, so a queryable copy of a group's database can be given to people who should not see patient data.
Redaction is an option on a replica rather than an intent of its own: one intent restores raw and redacted replicas alike, according to what each declaration asks for.

A replica redacts only when an operator declares that it does, and a declaration that says nothing does not redact.
A replica that does not redact serves the data as it was restored.

The parameters through which a consumer is told what to mask belong to Canopy for any intent carrying `redact`: an operator sets whether a replica redacts, not where its masking comes from.
A replica that does not redact is dispatched with those parameters unset.
Whether a replica redacts is therefore answered by its declaration alone, which is what makes an unredacted replica of a redacting declaration a finding rather than an ambiguity.

### The masking manifest

What to mask is a property of the product being restored (see [APP](../servers/application-types.md)) rather than a choice the operator makes per replica.
A *masking manifest* names the columns to mask and how each is masked, and a product Canopy can redact publishes one per version.

Canopy holds, for each such product, the location of its manifests as a template naming the version, together with the query that reads the application's own version out of the restored data.
Canopy resolves these settings into the worklist entry as it is dispatched, so a change to where a product publishes its manifests reaches every redacting replica without an operator revisiting a declaration.
The consumer resolves the version against the data it restored — the version of the snapshot, which is not necessarily the version the application reports running now — and fetches the manifest for it.

A redacting declaration contributes no worklist entry for a machine none of whose applications has a type with a manifest, and each such application surfaces as a gap on the declaration.
A replica that cannot be redacted is not restored at all: an unredacted replica standing in for a redacted one is worse than no replica.

Canopy corroborates a product's manifest template against the published artefacts it already holds per version.
A redacting declaration covering an application whose version has no published manifest is a gap, surfaced before a restore is attempted rather than discovered when one fails.
The manifest is resolved through the applications on the machine being restored, since what to mask is a property of the product in the snapshot rather than of the box that took it.

### What a redaction reports

Beyond the fields every report carries, a report for a redacting replica carries:

- the **redaction outcome** — fully applied, partially applied, or failed;
- the **manifest version** that was resolved and fetched;
- how many columns were **masked**, and how many were **skipped**;
- on a failure, a description of what went wrong.

A partial redaction is one where the manifest was applied but some of its columns could not be, leaving a replica that is live and mostly masked with an unidentified remainder in the clear.
A failed redaction is one where no masking took effect.

A consumer serves a replica only once its redaction has fully or partially applied, so a failed redaction leaves the replica on the data it was already serving.
The restore's health is reported as the consumer found it, independently of the redaction that follows: a snapshot that restored into a healthy database is reported healthy even when its redaction then failed, because the backup is good and the finding belongs to the redaction.
A redaction that fails is therefore reported when it fails, rather than withheld until a replica the consumer will not serve goes live.

Each replica's redaction outcome is presented alongside its restore health, so an operator reading the list of replicas sees which are redacted, which are only partly so, and which are serving data that was never masked.

## Alerting

A failed or overdue restore-health report raises a restore-verification check on the affected machine, subject to the same monitoring and incident gates as any other of that machine's checks.
It is a machine check because what failed to restore is the machine's backup, so it presents on every application on that machine as the machine's own (see [CHK](../monitoring/checks.md)).

A machine has one restore-verification check however many replicas it has.
Each replica is an instance of that check, graded on its own and carrying its type, its intent, its declared name, and the snapshot in the check's detail (see [CHK](../monitoring/checks.md)), so a rule or silence written for one replica applies to only that replica.
The detail names the replica by its type and intent together as well as separately, since a rule matches one variable at a time and a silence for a single replica has to pin both.
Two replicas of one type and intent on one machine are two instances, told apart by their names, so one of them failing leaves its siblings' results untouched.
The check reflects the most urgent of them, names the ones in trouble, and recovers when none is left degraded.

A replica's state is the worse of what its latest report said and whether it has gone past its overdue bound; these are one judgement about the replica, not two competing ones.
Canopy re-derives a machine's replicas on the same periodic sweep that decides overdue, from the declarations covering the machine and from the reports it holds about it.
A declaration covering the machine names one of its replicas whatever its consumer currently advertises: an intent that stops being advertised is a gap, and a finding standing against a replica does not go away because the consumer that reported it stopped offering to.
A replica the machine has a report for is one of its replicas too, for as long as a declaration still asks for that replica somewhere in the machine's group — which is what a consumer needs in order to report on it at all.
So a report about a machine its declaration does not name still surfaces against that machine, and a replica nothing declares any more stops being one of the machine's instances and needs no separate recovery: nothing can report on it again, so a finding held against it could never recover.
A report's effect appears on the next sweep rather than the instant it lands, which is immaterial for a warning that pages nobody.

A replica is also overdue — raising the same check on a periodic sweep, rather than waiting for a report that never arrives — when it has not met its intent's health expectation within the declaration's overdue bound.
For an intent carrying `once`, the expectation is measured against the latest snapshot: the replica is overdue when the latest snapshot has gone unverified for longer than the bound, not merely because time has passed since an earlier snapshot was verified.
For an intent without `once`, it is measured against wall-clock time since the last healthy report.
Overdue applies only to intents carrying `check`.

A failed migration test raises a migration-test check on the affected application, under the same gates, because that application is on the upgrade path to the version that failed.
It is an application check rather than a machine one because the version under test is the application's, so a box hosting two workloads carries a finding against whichever of them the version was tested for.
It is one check per application with its replicas as instances, as restore-verification is, and carries the target version in the detail rather than the name.
An application has one candidate at a time, so there is no second version whose result the first could mask, and a name per version would spawn a catalog policy per release.
A replica whose candidate has not been tried within its overdue bound degrades the same check: untested and failed are both "this version is not known good against this group's data".
A recorded verdict raises the check whatever declares that replica now, and a later verdict is what supersedes it: what a version's migrations did to a group's data is a fact about the version, not something a declaration has to keep asking about for it to remain true.

The check is a warning rather than a failure, and does not escalate.
Nothing is wrong with the live application: it is running the version it always was, serving patients, and the finding is about a version it has not taken yet.
Treating it as a failure would open an incident against a healthy group and put a migration problem in front of whoever is on call for outages, when the people who need it are the ones deciding whether that version ships.
The version's readiness is where the finding does its work.

A redaction that did not fully apply raises a redaction check on the affected machine, under the same gates.
It is one check per machine with its redacting replicas as instances, as restore-verification is, each carrying the redaction outcome, the manifest version, and the counts of masked and skipped columns in the check's detail.
A replica is an instance of it only once it has reported a redaction outcome: a declaration that redacts but has produced no replica yet has nothing unmasked to report.
An instance recovers when the replica's next report redacts fully, and the check when none of them is left degraded.

The check is a warning rather than a failure, and does not escalate.
The group is healthy and its data is where it should be; the finding is that a replica made from that data is not as safe to hand out as it was declared to be.
That is for whoever gave out the replica to act on, not for whoever is on call for outages.

## Out of scope

- How a consumer provisions, runs, names, or tears down a replica, or how it applies migrations or a masking manifest to one.
- A consumer's runtime placement, storage sizing, or scheduling.
- Producing reporting schemas, or any other artefact, from a migrated replica.
- The contents of a masking manifest, and what each masking it names does to a value.
- Deciding or scheduling when a group upgrades: verdicts inform that decision without making it.
- Scoping object-storage credentials below the granularity of a group's repo: one repo holds all of a group's machines' snapshots, so credentials are necessarily group-wide while targeting and reporting are per-machine.
- Longer-lived or non-chained credentials: a consumer refreshes within a restore, so the per-issuance lifetime is not a constraint.
