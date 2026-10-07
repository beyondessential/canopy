# Database connection resilience during primary failover

Scenarios verifying `database::Db::get()` retries a transient checkout failure instead of surfacing it on the first attempt, per [DBR](../../specs/platform/database-resilience.md).

## Checkout behaviour

- [x] A pool with a live database checks out a connection on the first attempt, with no retry. Verifies spec: DBR. (`database::tests::get_succeeds_on_a_healthy_pool_without_retry`)
- [x] The checkout retry schedule's cumulative wait is a bounded, pinned total rather than open-ended, and sits inside the ~20s failover window the spec describes. Verifies spec: DBR. (`database::tests::checkout_backoff_window`)
- [x] A checkout against a database that is still unreachable once the retry window is exhausted fails with the same error shape a non-retrying pool would have produced on its first attempt. Verifies spec: DBR. (`database::tests::get_exhausts_retries_and_fails_like_a_non_retrying_pool`)
- [ ] A checkout that fails on an early attempt but succeeds once the primary becomes reachable again (mid-retry recovery) proceeds normally, with nothing in the response indicating a retry happened. Verifies spec: DBR. Not covered — exercising this needs a backend that can be made to fail and then start accepting connections mid-test (e.g. a toxiproxy-style fault injector or a throwaway listener that starts refusing then starts accepting); no such harness exists in this repo today.
- [ ] Each retried attempt logs a WARN with the attempt number and the error; a first-attempt success logs nothing. Not covered — would need a tracing test subscriber capturing emitted events; the retry-and-exhaust test only asserts on the returned error, not on log output.
