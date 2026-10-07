# Database connection resilience during primary failover

Scenarios verifying `database::Db::get()` per [DBR](../../specs/platform/database-resilience.md).

## Checkout behaviour

- [x] A pool with a live database checks out a connection on the first attempt. Verifies spec: DBR. (`database::tests::get_succeeds_on_a_healthy_pool`)
- [x] A checkout against a server that stays unreachable keeps retrying until the retry window has passed, stays within the overall limit, and then fails with the same error a single attempt produces. Verifies spec: DBR. (`database::tests::an_unreachable_server_is_retried_for_the_window_then_fails_as_without_retry`)
- [x] A checkout that times out waiting for a connection another request holds is reported at its limit, not retried. Verifies spec: DBR. (`database::tests::waiting_for_a_busy_pool_is_not_retried`)
- [x] A connect to a server that accepts the connection and never answers (a dropped node rather than a refused port) counts as a failed attempt after the per-attempt limit. Verifies spec: DBR. (`database::tests::a_server_that_never_answers_fails_the_attempt_at_its_limit`)
- [x] A checkout that fails on an early attempt but succeeds once the primary becomes reachable again (mid-retry recovery) proceeds normally on a working connection. Verifies spec: DBR. (`database::tests::a_server_that_comes_back_within_the_window_is_reached`)
- [ ] Each retried attempt logs a WARN with the attempt number and the error; a first-attempt success logs nothing.

## Connection reuse

- [x] A connection returned in good order is kept idle and served to the next checkout. Verifies spec: DBR. (`database::tests::a_returned_connection_is_reused`)
- [x] A connection returned while still inside a transaction is closed rather than kept. Verifies spec: DBR. (`database::tests::a_connection_left_in_a_transaction_is_discarded`)
- [ ] An idle connection whose server has gone away fails its check-out health check and is replaced with a fresh connection, without the request seeing an error. Verifies spec: DBR.
