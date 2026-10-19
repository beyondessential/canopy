---
id: AUD
---

# Audit log

Canopy records who did what across the operator surface, the device API, the fleet query interface, and its own background work.
The log is written as a condition of the action it records, and is archived offsite once it ages out of Canopy's own database.

## What is recorded

The log records actions: what was done, by whom, to what.
It is not a log of requests, so an entry says what happened rather than what a caller sent.
This spec covers what holds for every entry; which actions a feature records, and what each holds, is that feature's to say.

Every handler on the operator surface, the device API, and the fleet query interface declares whether it is audited or audit-silent, and a handler that declares neither fails the build.
Being silent is therefore a deliberate choice rather than something a new handler slips into.
An audited handler declares the actions it takes, and records an entry for each action it takes.
One request can take several actions, as a bulk change does, and then records an entry for each.
An audited handler that completes without recording an action, or records one it did not declare, fails, and what it changed is rolled back.
A silent handler never records an entry.

Whether a handler is audited is independent of its safety grade (see [SAFE](../private-server/safety-modes.md)).
A read can be audited, as running a query in the SQL playground is, and a change can be silent.

On the device API, the handlers that issue a credential or trust material, or that change Canopy's records beyond reporting status, are audited.
Reporting status and reading are silent.

Canopy's own background work records an entry where a job declares the change it makes worth recording.

An action that errors is rolled back and leaves no entry, unless it committed an entry ahead of the rest of its work (see "Writing as a condition of the action").

## Actions

Every entry names its action from Canopy's catalogue of actions, such as revoking a certificate, declaring a maintenance window, or issuing restore credentials.
An action names a kind of thing done, never one instance of it: what varies between instances is in the entry's detail.
An action's name is stable, and does not change when the operator interface or an API is reorganised, so a search by action reaches the whole log, the archive included.
An action is recorded the same way whichever surface it is taken through.

## Actors

Every entry names its actor, which is one of:

- an operator, by their tailnet login;
- a device, presented by its role as a machine device, a relay device and so on (see [DTR](../private-server/device-trust.md)), and the application it acted for where it acted for one;
- a fleet query access token, by the token (see [MCP](../private-server/mcp.md));
- Canopy itself, by the job that acted.

A device's enrolment is its own action, its actor being the device the enrolment creates.

## What an entry holds

An entry holds when it happened, its actor, its action, the records it acted on, and the action's detail.
The records acted on are the machines, groups, applications, configurations, credentials and the like that the action concerned, and an action that creates a record names the record it created.
The detail is what the action defines as worth knowing about it: the reason given for a revocation, the intent a lease was taken with, what the action produced.
Nothing reaches an entry that its action does not define.

An entry holds identifiers, never a secret.
A credential issued is recorded by its identifier, and no action holds a secret in its detail.

Canopy can look entries up by the records they acted on and by values in their detail, and count them by those values over a window of time.

## Writing as a condition of the action

An audited action is not carried out unless its entry is written.
The entry and the change it records are committed together, so either both stand or neither does.

A handler can commit an entry ahead of the rest of its action, so that the entry stands whatever becomes of the action.
An entry committed ahead is pending until the action settles it, once, as done or failed.
Settling it done commits together with the rest of the action's work; settling it failed commits on its own, since that work is rolled back.
An action settles its entry failed only when it knows its effect did not take place.
Settling can add to the entry's detail what the action has learned since, such as the identifier of a credential an outside service issued.
A pending entry that is never settled is presented as unconfirmed, because what it records may have taken place even though the action did not complete.
Once its week is archived an entry can no longer be settled, and one still pending is archived as unconfirmed.

Committing ahead is how:

- an action with an effect outside Canopy's own records, such as issuing cloud credentials or changing something on a cluster, is recorded before the effect;
- an audited read is recorded before it reads, so a read that fails is recorded as failed;
- a feature records a decision it wants kept when the action does not go ahead, such as a refusal with its reason, where that feature's spec calls for one.

## Keeping entries

Canopy's database holds the recent log, stored in weekly ranges provisioned ahead like the other histories (see [HST](history-storage.md)).
An entry stays there for ninety days.
An action can be kept for a year instead, where its entries are few and matter long after the fact.
An entry leaves the database only once its retention has passed and its week has been archived; an unarchived week is never removed, however old.

## Archive

The archive is where the log is kept for good, rather than a copy of it.
It is written to the recovery escrow's storage (see [ESC](../jobs/escrow.md)) under its own prefix, and is not encrypted to the escrow recipients, since Canopy reads it back.

The archive is written one closed week at a time, as one object per week, so periods never overlap and every entry is archived exactly once.
Each week's object holds every entry recorded in that week, whatever its retention in the database.
The object is in Parquet format, so that it can be read without Canopy.

A week counts as archived once its object has been written, read back and checked against what was sent, and is under object lock.
Until then, none of its entries leave the database.

Each week's object records the hash of the previous week's object, so a missing or substituted week breaks the chain and shows.

Archived weeks are kept indefinitely.
Object lock protects each one for the escrow storage's required retention, and the object remains after the lock expires.

## Reading the log

Any administrator can read the whole log from the operator interface.
The log page lists entries newest first, and can be filtered by actor, action, time, and the records acted on.
A time range reaching back past what the database holds is answered from the archive, fetching the weeks it covers.

The fleet query interface offers the log to any caller it serves, over what the database holds.

## Self-alerts

The log's weekly ranges are provisioned ahead and alerted on as they run short, as for every history (see [HST](history-storage.md), "Running short").
Running out matters more here than for the other histories, because without a range to write into no audited action can be carried out.

Canopy raises a self-alert (see [SELF](../private-server/self-alerts.md)) when archiving falls behind.
It warns when a closed week has gone a day without being archived, and fails when an unarchived week reaches the end of the ninety days, since from then on the database is holding entries it would otherwise have let go.
It names the weeks outstanding, and clears once every closed week is archived.
