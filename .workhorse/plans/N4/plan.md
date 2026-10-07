# Retry transient DB connection failures during primary failover

Implements [DBR](../../specs/platform/database-resilience.md).

## Context

Confirmed in prod today: a CNPG primary switchover (triggered by an AWS spot interruption reclaiming the primary's node — infra-level mitigation tracked separately, out of scope here) produced a ~20s window where `private-server` and `public-server` returned 500s, logging `no database connection: error connecting to server`.

`database::Db` is `pub type Db = Pool<AsyncPgConnection>` (a `diesel_async::pooled_connection::mobc::Pool`). Every handler does `state.db.get().await?` or `state.db_read.get().await?`; the error (`mobc::Error<PoolError>`) converts straight to `AppError::DatabasePool` and a 500 via `#[from]` in `crates/commons-errors/src/lib.rs`. mobc's own pool only retries `Error::BadConn` (idle-connection revalidation); a fresh connect failure — exactly what a mid-switchover connection attempt is — returns on the first try.

Across the codebase, the only methods ever called on a `Db` value are `.get()` and `.clone()` (confirmed by grep across `crates/`). So the fix is a **newtype around the pool with its own retrying `.get()`**, not a per-call-site change: every one of the ~230 existing `state.db.get().await?` / `pool.get().await?` call sites keeps compiling unchanged.

## Steps

- [x] Add `mobc = { workspace = true }` to `crates/database/Cargo.toml` (`[dependencies]`). It's already pinned in the workspace root `Cargo.toml` (`mobc = "0.9.0"`) and already pulled in transitively via `diesel-async`'s `mobc` feature; this just lets `crates/database/src/lib.rs` name `mobc::Error` directly. `commons-errors` already depends on it directly for the same reason, so this is consistent with the existing pattern.

- [x] In `crates/database/src/lib.rs`, change `pub type Db = Pool<AsyncPgConnection>;` to:
  ```rust
  #[derive(Clone, Debug)]
  pub struct Db(Pool<AsyncPgConnection>);
  ```
  `mobc::Pool` implements both `Clone` and `Debug` itself, so the derives are free. Update `build_pool()` to construct `Db(Pool::builder()...build(...))` instead of returning the bare pool.

- [x] On the same `Pool::builder()` in `build_pool()`, add an explicit short `.get_timeout(Some(Duration::from_secs(3)))`. Without this, mobc's default 30s `get_timeout` wraps *each* retry attempt individually — if a connect attempt hangs rather than failing fast (e.g. a silent network drop, not a clean connection-refused), the retry loop below could take minutes instead of seconds. Capping each attempt at 3s keeps the loop's worst case bounded and predictable.

- [x] Add the retrying `get()` method on `Db` in the same file:
  - A module-level `const CHECKOUT_RETRY: Backoff` (from `commons_types::backoff::Backoff`) and a max-attempts constant. Size them so the cumulative wait comfortably covers the ~20s failover window observed today while staying clear of the 3s-per-attempt timeout above — e.g. base 250ms, cap 2s, around 9–10 attempts (work out the exact cumulative total in a unit test the way `application_certificates.rs`'s `backoff_grows_then_settles` test does, rather than hand-computing it here).
  - `pub async fn get(&self) -> Result<diesel_async::pooled_connection::mobc::PooledConnection<AsyncPgConnection>, mobc::Error<PoolError>>` — loop calling `self.0.get().await`; on `Err`, if attempts remain, sleep for `CHECKOUT_RETRY.after(attempt)` (convert the `jiff::SignedDuration` to a `std::time::Duration` for `tokio::time::sleep` — check jiff's conversion API, there's precedent for `SignedDuration` round-tripping elsewhere in the codebase but not yet a sleep-on-it case) and retry; once attempts are exhausted, return the last error unchanged so the eventual `AppError::DatabasePool` is identical to what it would have been without retrying.
  - Log each retry at `WARN` (attempt number, the error) so an operator watching logs during a real outage can see the pool recovering rather than wondering why requests got slower. Don't log anything on first-attempt success — only once a retry has actually happened.
  - `PoolError` needs importing from `diesel_async::pooled_connection`.

- [x] Check `crates/database/src/issues.rs` and any other `crates/database/src/*.rs` module that takes `&Db` or constructs test pools directly (not through `init()`/`init_to()`) — grep turned up `crates/database/src/issues.rs` and `crates/database/tests/it/partitions.rs` using `pool.get()` already through the existing `Db` type, so they need no change, but confirm nothing in `database`'s own source reaches into the pool a different way (e.g. via `Pool::builder` directly) that the type change would break.

- [x] `cargo check --workspace` (via `just check`) to catch anything the grep missed — in particular anywhere that expects `Db` to literally be `mobc::Pool<...>` (e.g. a generic bound, a `From`/`Into` impl, or a place passing `state.db` into a function written against the diesel_async pool type rather than `database::Db`). None were found by grep (`Pool<AsyncPgConnection>` and `Pool::builder`/`AsyncDieselConnectionManager` only appear in `database/src/lib.rs`), but the compiler is the real check.

- [x] Add a unit test in `crates/database/src/lib.rs` (or a new `tests/it/db_retry.rs`) asserting: (a) `get()` on a healthy pool succeeds without any retry/log noise, (b) the backoff schedule's cumulative wait lands in the intended ~15-20s range (the arithmetic test, not a real failing Postgres), and (c) `get()` against a pool pointed at an address nothing listens on eventually returns the same `Err` shape as the bare pool would, after exhausting attempts — bound this test's own wall-clock by using a much shorter `Backoff`/attempt-count passed through a non-default constructor or `cfg(test)` override, so the test suite doesn't spend 20 real seconds on it.

- [x] Run `just test-package database`, then `just test-package private-server` and `just test-package public-server` to confirm the whole workspace still compiles and passes against the new `Db` type.

- [x] `cargo fmt` the workspace.

## Explicitly not doing here

- Not adding a `/readyz` that checks DB connectivity — the user scoped this card to the retry fix only; the lack of a DB-aware readiness probe is a separate concern.
- Not touching `ops/pulumi` / moving `meta-db` off the spot node pool — tracked as a separate decision outside this card.
