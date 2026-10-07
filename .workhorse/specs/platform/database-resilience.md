---
id: DBR
---

# Database connection resilience

Canopy's private and public servers reach their own database through a pool of connections to its current primary.
A request that needs a connection during a brief loss of that primary succeeds instead of failing, because the attempt is retried for a short window rather than reported on the first failure.

## The guarantee

A connection attempt that fails because the primary is unreachable is retried, with the wait between attempts growing each time, until a connection succeeds or a bounded overall window elapses.
The window is short enough that an operator can still tell a passing failover from a database that is actually down, and sits well inside the latency a client already tolerates from an ordinary slow request.
A request that acquires a connection within the window proceeds and returns normally, with nothing in the response indicating a retry happened.

## A still-down database fails the same way

A failure that persists past the retry window is reported exactly as a connection failure without retry would be: the same error, the same status, nothing marking it as having been retried.
So logging, alerting, and client error handling are unchanged; a database that is genuinely down is still reported as down, only after the window rather than on the first attempt.
