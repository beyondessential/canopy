---
id: AUD
---

# Audit log

Canopy records who did what across the operator surface, the device API, the fleet query interface, and its own background work.
The log is written as a condition of the action it records, and is archived offsite once it ages out of Canopy's own database.

## What is recorded

Every handler on the operator surface, the device API, and the fleet query interface declares whether it is audited or audit-silent, and a handler that declares neither fails the build.
Being silent is therefore a deliberate choice rather than something a new handler slips into.
An audited handler produces an entry each time it acts; a silent one never does.

Whether a handler is audited is independent of its safety grade (see [SAFE](../private-server/safety-modes.md)).
A read can be audited, as running a query in the SQL playground is, and a change can be silent.
An entry carries nothing of safety modes: no grade, no mode, and no session.

On the device API, the handlers that issue a credential or trust material, or that change Canopy's records beyond reporting status, are audited.
Reporting status and reading are silent.

Canopy's own background work produces an entry where a job declares the change it makes worth recording.

An action that errors is rolled back and leaves no entry, with one exception: an action with an outside effect (see "Writing as a condition of the action").

## Actors

Every entry names its actor, which is one of:

- an operator, by their tailnet login;
- a device, by its machine, and the application it acted for where it acted for one;
- a fleet query access token, by the token (see [MCP](../private-server/mcp.md));
- Canopy itself, by the job that acted.

## Refusals

A caller refused for authorisation produces a refusal entry, naming the caller as far as it was identified, the operation it asked for, and the reason it was refused.
This covers a caller who is not an administrator reaching an audited operator handler, and a device refused something it asked for, such as a credential it holds no grant for.
A request refused for its safety mode produces no entry.

## What an entry holds

An entry holds when it happened, its actor, the operation, and those of the request's arguments the handler declares as recorded.
Nothing else from the request is recorded, so an argument reaches the log only by being declared.
The declared arguments are what make an entry say what it acted on: the machine, the group, the configuration.

An entry holds identifiers, never a secret.
A credential issued is recorded by its identifier, and a secret argument is never declared as recorded.

Entries can be looked up by the values of their recorded arguments, and counted by those values over a window of time.

## Writing as a condition of the action

An audited action is not carried out unless its entry is written.
The entry and the change it records are committed together, so either both stand or neither does.

An action with an effect outside Canopy's own records, such as issuing cloud credentials or changing something on a cluster, records its entry as pending before the outside effect, and confirms it once the effect has taken place.
The confirmation is a record of its own, pointing at the pending entry, so the log is only ever appended to.
A pending entry that is never confirmed stands, and is presented as unconfirmed, because its outside effect may have taken place even though the action did not complete.

## Keeping entries

Canopy's database holds the recent log, stored in weekly ranges provisioned ahead like the other histories (see [HST](history-storage.md)).
An entry stays there for ninety days.
A handler's declaration can keep its entries for a year instead, for entries that are few and matter long after the fact.
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
The log page lists entries newest first, and can be filtered by actor, operation, time, and the thing acted on.
A time range reaching back past what the database holds is answered from the archive, fetching the weeks it covers.

The fleet query interface offers the log to any caller it serves, over what the database holds.

A page that shows who did something to a record reads it from the log, and shows it for as long as the entry is held in the database.
After that, the page shows when it happened, and the log page answers who.

## Self-alerts

The log's weekly ranges are provisioned ahead and alerted on as they run short, as for every history (see [HST](history-storage.md), "Running short").
Running out matters more here than for the other histories, because without a range to write into no audited action can be carried out.

Canopy raises a self-alert (see [SELF](../private-server/self-alerts.md)) when archiving falls behind.
It warns when a closed week has gone a day without being archived, and fails when an unarchived week reaches the end of the ninety days, since from then on the database is holding entries it would otherwise have let go.
It names the weeks outstanding, and clears once every closed week is archived.
