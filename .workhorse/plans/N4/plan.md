# Retry transient DB connection failures during primary failover

Implements [DBR](../../specs/platform/database-resilience.md).

## Context

A CNPG primary switchover (triggered by an AWS spot interruption reclaiming the primary's node; infra-level mitigation tracked separately, out of scope here) produced a ~20s window where `private-server` and `public-server` returned 500s, logging `no database connection: error connecting to server`.

Every handler does `state.db.get().await?`; the error (`mobc::Error<PoolError>`) converts straight to `AppError::DatabasePool` and a 500. The only methods ever called on a `Db` are `.get()` and `.clone()`, so the fix is a newtype around the pool with its own retrying `get()`, and no call site changes.

### diesel-async's mobc manager discards healthy connections

diesel-async 0.9.2's mobc `Manager::validate` returns `panicking() || is_broken()`, but mobc keeps a connection on check-in only when `validate` returns true. So every healthy connection was closed after use, and every broken one (including one left mid-transaction by a cancelled request) went back to the idle list. Every checkout was therefore a fresh connect, which is why the failover surfaced as connect errors on every request. A connection put back while still inside a transaction passes the `SELECT 1` health check on its next checkout, so the next request's writes would land in a transaction that never commits.

`database::ConnectionManager` wraps diesel-async's manager and corrects `validate`. `a_returned_connection_is_reused` and `a_connection_left_in_a_transaction_is_discarded` both fail against the upstream `validate`. Worth reporting upstream; drop the wrapper's `validate` once a fixed diesel-async is pinned.

### Timeouts

Three limits, each doing one job:

- `SERVER_TIMEOUT` (3s) on each connect and each check-out health check, inside the manager. A dropped node never answers, and without this an attempt waits for the OS TCP timeout.
- `CHECKOUT.retry_window` (20s): no retry starts after this long. Covers the observed failover.
- `CHECKOUT.timeout` (30s, mobc's default): the whole checkout, queue wait and retries included. A request waiting on a busy pool waits as long as before.

Only `mobc::Error::Inner` (failed connect) is retried. `Timeout` means the request already waited its full limit for a connection another request holds; retrying it would take it out of mobc's FIFO queue and put it back behind newer requests. `PoolClosed` never recovers.

## Steps

- [x] `Db` newtype over `mobc::Pool<ConnectionManager>`; `PooledConnection` alias for the checkout type
- [x] `ConnectionManager`: delegate `connect`/`check` with `SERVER_TIMEOUT`; corrected `validate`
- [x] `Db::get` with a deadline-bounded retry loop over `Error::Inner` only
- [x] Tests: healthy checkout, reuse, mid-transaction discard, unreachable server exhausts the window, busy pool not retried, silent server times out the attempt, server returning mid-window is reached
- [x] Remove the stability backfill (its session-scoped advisory lock blocks moving to a transaction-mode pooler) with a migration dropping `check_stability_backfill`
- [x] `cargo check --workspace --tests`
- [x] `just test` for `database`, `private-server`, `public-server`, `jobs`

## Not doing here

- A `/readyz` that checks DB connectivity: a separate concern.
- Moving `meta-db` off the spot node pool: tracked separately.
