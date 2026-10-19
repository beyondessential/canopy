---
id: HST
---

# History storage

Canopy keeps three high-volume histories: every ingested status push (see [STA](../public-server/statuses.md)), every authenticated device connection, and the audit log (see [AUD](audit-log.md)).
All three are stored in weekly ranges, so a read bounded to a recent window touches only the weeks that window covers.

## Provisioning ahead

A history write succeeds only while a range covering its timestamp exists, so Canopy provisions ranges ahead of time rather than at the moment first data needs one.
Canopy keeps at least four weeks of future ranges provisioned for each history, and maintains them for as long as it is running rather than on an external schedule.

Provisioning is idempotent: applied over ranges that already exist it changes nothing, so it can be run at any time and by more than one component at once.
Provisioning a range does not block reads or writes of the history it extends — a status push, a connection record, or an audit entry is never delayed by it, and a long-running reader never delays it.

## Running short

Canopy reports how much future range each history has left, and raises a self-alert (see [SELF](../private-server/self-alerts.md)) as it runs short: a warning below two weeks remaining, and a failure below one week, because a history with no range left cannot be written at all.
The alert names each history that is short and the date its ranges run out, and clears once every history is provisioned ahead again.

## Recording a connection

Recording an authenticated device connection is part of accepting the request, not a condition of it.
When the record cannot be written the request proceeds regardless, and authentication does not fail on account of the history being unwritable.
