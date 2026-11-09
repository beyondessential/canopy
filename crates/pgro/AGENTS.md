# pgro

The restore operator: a Kubernetes operator that Canopy drives to restore backups into Postgres replicas.
Its specs are in `.workhorse/specs/restore-operator/` (ids `PGRO`, `INT`, `PLACE`, `USER`); tie code to them with `// spec: INT` comments.
In those specs and in this file, "operator" outside the name of the crate means a person, and the software is pgro.
These rules add to the ones in the repository's root `AGENTS.md`.

## Working on it

- When adding or changing features, or when fixing bugs, add tests whenever possible.
- Run unit tests with `just test -p pgro --lib <filter>`.
- The cluster tests in `tests/it/` are ignored by default and only run in CI, against a kind cluster with MinIO and kopia. Tell the user when you add one, add its module to the matrix in `.github/workflows/pgro-integration.yml`, and do not run them locally.
- When adding or changing items in the CRD types, run `just gen-crds` and change the tables in `README.md` to match.
- pgro reaches Canopy through the tailnet transport in `src/canopy/transport.rs` and the generated client in `crates/canopy-api`. Nothing else carries a Canopy call.
- If you can do something from pgro instead of in a script that runs in a Job, do so. If you need to query something in a database, pgro should be able to do it, instead of SQL written in shell logic.
- When removing code that has already been committed, delete it unless explicitly asked to comment it out.
- Prefer small dependencies to reimplementing the wheel. Ask the user to pick one if there is no obvious choice.

## Code style

- Imports: merge them and group them by std, then third-party and workspace, then local (`crate`, `super`, `self`). `use` statements go before `mod` statements.
- Use the newer `foo.rs` and `foo/sub.rs` style of modules.
- When writing parsers, unless very trivial, implement them using winnow.
- To silence a warning, use `#[expect(..., reason = "...")]` instead of `#[allow(...)]`.

## Lessons learned

- Detect dead Postgres connections with **TCP keepalives** (`socket2::TcpKeepalive` on the raw `TcpStream` before handing it to `tokio_postgres::Config::connect_raw`), not with a wall-clock `tokio::time::timeout` around the query. Keepalives fire on the kernel's own timer regardless of in-flight queries, so a legitimately long-running statement (e.g. `DROP SCHEMA … CASCADE` on a populated dbt schema, which can run for tens of minutes) keeps working, while a NAT-evicted or silently dead socket still errors within about 90s and the reconcile retries.
- Wall-clock timeouts on a long-tail-distributed operation make a retry loop **never converge**: every attempt hits the cap, error policy retries, every retry hits the cap again. Don't add a timeout unless you have a real p99 number for the operation it wraps.
- pgro's own libpq sockets are separate from the migration Job's libpq URI. Keepalive fixes in the Job (`keepalives=1&keepalives_idle=...`) do **not** cover pgro: set both.
- The kube port-forward path is not a raw TCP socket, so socket-level keepalives don't apply. That path keeps its own liveness via the API server WebSocket, and only `connect_via_tcp` needs the socket2 setup.
