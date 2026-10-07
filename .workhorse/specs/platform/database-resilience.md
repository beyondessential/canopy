---
id: DBR
---

# Database connection resilience

Canopy's private and public servers reach their own database through a pool of connections to its current primary.
A request that needs a connection during a brief loss of that primary succeeds instead of failing, because the attempt is retried for a short window rather than reported on the first failure.

## The guarantee

A connection attempt that fails because the primary is unreachable is retried, with the wait between attempts growing each time, until a connection succeeds or the retry window has passed.
The window covers the primary failovers seen in production, and is short enough that an operator can still tell a passing failover from a database that is actually down.
Each attempt to reach the server is limited in time, so a server that has stopped answering counts as a failed attempt promptly rather than holding the attempt open.
The whole of a request's wait for a connection, retries included, has a fixed upper limit.
A request that acquires a connection within the window proceeds and returns normally, with nothing in the response indicating a retry happened.

## Waiting for a busy pool

A request that finds every connection in use waits for one to be returned, up to the same upper limit.
That wait is not a failure to reach the primary, so running out of it is reported straight away rather than retried.

## A still-down database fails the same way

A failure that persists past the retry window is reported exactly as a connection failure without retry would be: the same error, the same status, nothing marking it as having been retried.
So logging, alerting, and client error handling are unchanged; a database that is genuinely down is still reported as down, only after the window rather than on the first attempt.

## Connection reuse

A connection a request returns in good order goes back to the pool and serves later requests.
A connection returned broken, or still inside a transaction because its request ended part-way through, is closed and never handed to another request.
A pooled connection is checked against the server before it is handed out, and one that fails the check is replaced with a fresh connection.
