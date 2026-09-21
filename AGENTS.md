<!-- BEGIN:workhorse 0.3.0 -->
# Workhorse framework

This workspace uses [Workhorse](https://github.com/beyondessential/workhorse), a spec-driven development workbench. Workhorse ships skills (invokable prompts) and reference docs into this repo to shape how AI agents work here.

- **Skills** live at `.agents/skills/` — each skill is a folder containing a `SKILL.md` with YAML frontmatter and a prompt body. `.claude/skills/` is a symlink to the same folder so Claude Code picks them up natively
- **Reference docs** live at `.agents/docs/` — long-form guidance that skill bodies cite by path (spec format conventions and similar)
- **Specs** live at `.workhorse/specs/` — acceptance criteria for each piece of work, organised into areas by subdirectory

When picking up a task, read the skill whose folder name matches what you're being asked to do — its `SKILL.md` describes how to approach the work and which reference docs to follow.

Workhorse keeps this section, the skills, and the reference docs current automatically: the first agent turn of a session smart-merges the latest release over your local edits, so your deliberate changes survive. Edit or remove it freely.
<!-- END:workhorse -->

# Agent Rules for canopy

Avoid writing large summaries of actions taken when done.

## Project Structure Overview
- **Database crate**: Models, migrations, and database logic
- **Public server**: Internet-exposed API endpoints for device registration and updates
- **Private server**: Admin HTTP/JSON API in axum, plus an embedded React SPA (in `private-web/`) served at the root in production
- **Commons**: Shared utilities, authentication, error handling

## Database Connection
- Database models are in `crates/database/src/` with re-exports in `lib.rs`
- Use migrations in `migrations/` directory for schema changes
- Create new migrations with `just migration NAME` — never hand-create migration files/directories, as that produces inconsistent naming
- Use `just migrate` to run migrations
- Use PostgreSQL native functions where possible (e.g., `position()` for binary substring searches)
- Complex queries: Prefer database-level operations over in-memory filtering

## Code Style & Patterns
- Follow existing Rust conventions in the codebase
- Never add useless comments if what the code does is obvious enough, especially in CSS
- Use `AppError` variants for error handling, map to appropriate HTTP status codes in `IntoResponse`
- Update ERRORS.md when adding new error types, the heading must match the error problem type
- Use `commons_tests::db::TestDb::run()` for database-only tests
- Use `commons_tests::server::run()` for HTTP endpoint tests
- Use `commons_tests::server::run_with_device_auth()` for authenticated device tests in the public server
- Admin endpoints take a `TailscaleAdmin` axum extractor; the React UI gates with `commons.is_current_user_admin`
- **Check-state / issue / policy scope** is the single `database::issues::Scope` enum (`Application(id)`, `Machine(id)`, `Group(id)`, `Global`) — use it for filing (`CheckFiling.scope`, with `device_id` a *separate* provenance field), scoped silences (`ScopedCheckPolicy`), and incident-target resolution. Never add another scope enum or hand-write `match` over the storage columns; map through `Scope::from_columns` / `to_columns` / `resolve_incident_target`. Storage stays one nullable FK column per targetable grain (`application_id`, `machine_id`, `server_group_id`, all null being canopy-wide), with a `num_nonnulls` CHECK holding it to at most one, so Postgres keeps the `ON DELETE CASCADE` + uniqueness that prevents orphaned check-states. Adding a grain means adding a variant, a column, and an arm to each of those three functions, never a second enum.
- **Backup checks never default to a failure.** `Failed` in canopy means a live service is down and gets a fast human response; a late, unreconciled, or unverified backup is not that, and the fleet's backups are layered. Every check in the backup sphere (`backup-*`, `preflight-*`, `restore-verification`, `redaction`, `migration-test`) ships with `default_ceiling: CheckResult::Warning` and `default_escalates: false`. The only exceptions are `backup-corruption`, `backup-rotation-broken`, and `preflight-object-lock`, where the backups are already gone, unrecoverable, or unprotected. Operators raise an individual check to a failure through its policy; that is their call, never a shipped default. Note `escalates` is inert unless the ceiling is `Failed` (`escalates_normalised`). See the alerting section of `.workhorse/specs/jobs/backup.md`.
- **Say group, environment, rank — never "deployment".** A group is what Canopy attaches shared state to (see `.workhorse/specs/servers/groups.md`); a group's applications at one rank are one of its environments; an installation of Canopy itself is "the Canopy instance". The word survives only in the `billing.deployment` cost-allocation label, read by AWS cost allocation and by every machine reading its effective tags, so `grep -rin deployment` over `.workhorse/specs`, `crates`, `private-web/src` and `private-web/e2e` should turn up only that label, the `BillingLabels.deployment` field carrying it, and Kubernetes `Deployment` resources. Migration SQL is exempt: an applied migration is history.
- **Never encode a parameter in a check name.** A check name is a category operators configure once — `backup-staleness`, not `backup-staleness:tamanu-postgres`. Anything that varies per instance (backup type, restore intent, configuration name) goes in the filing's `detail`, where scoped policy rules reach it as `check.<field>` and operators can grade individual instances differently without a catalog row each. A parameterised name multiplies one alert into fifteen and defeats the point of the catalog.
- **The public API stays compatible unless a break is coordinated.** The bar is mechanical: `bes-canopy-api` (the published client, generated from `crates/public-server/openapi.json` into `crates/canopy-api/` by `just gen-api`) after your change must not be a semver-breaking change against the crate generated from it before, per `cargo-semver-checks`. So: never remove a path, schema, property, or enum variant; never make a request property required that was optional; never change a property's type, including widening it to nullable. Rename by *adding* the new name and keeping the old — a response emits both, a request accepts either and requires neither, and a request naming both is a 400 rather than a guess. Reserved tag keys (`canopy:*`) are held to the same rule despite being invisible to the check, since a consumer cannot tell a withdrawn key from an absent value. The private API is the SPA's and ships with it, so it is exempt. After changing the public API run `just gen-openapi && just gen-api` and commit both the document and the regenerated client; CI's `just check-generated` fails when either is stale. See `.workhorse/specs/platform/api-compatibility.md` and `.workhorse/specs/platform/api-client-crate.md`.

## Private server architecture
- **Server fns** under `crates/private-server/src/fns/<module>.rs` are bare axum handlers with `(State, [auth extractor], Json<Args>) -> Result<Json<T>>` signatures.
- Each module exposes `pub fn routes() -> Router<AppState>` and is mounted under `/api/<module>` by `crate::fns::routes()`.
- The SPA fallback (`crate::spa::handler`) serves the embedded React bundle from `private-web/dist/` for any path the API doesn't claim.
- `build.rs` runs `npm install --frozen-lockfile && npm run build` in `private-web/` before embedding. Set `SKIP_FRONTEND_BUILD=1` to skip (`just`'s recipes already do this for dev workflows).

Example axum handler pattern:
```rust
#[derive(Deserialize)]
pub struct AddArgs { pub email: String }

pub async fn add(
    State(state): State<AppState>,
    TailscaleAdmin(_): TailscaleAdmin,
    Json(args): Json<AddArgs>,
) -> Result<Json<()>> {
    let mut conn = state.db.get().await?;
    database::admins::Admin::add(&mut conn, &args.email).await?;
    Ok(Json(()))
}
```

## Testing Patterns
- Use `#[tokio::test(flavor = "multi_thread")]` for async tests
- Database tests: `commons_tests::db::TestDb::run(|mut conn, _url| async move { ... })`
- HTTP tests: `commons_tests::server::run(|conn, public, private| async move { ... })`
- Device auth tests: `commons_tests::server::run_with_device_auth("role", |conn, cert, device_id, public, private| async move { ... })`
- In public-server, authenticate a test request with the client-certificate header the harness trusts. Only one header is read, chosen per server; the test harness selects Envoy/XFCC, so it is `.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))`. A test that needs the nginx path instead (`mtls-certificate`) selects it explicitly with `commons_tests::server::run_with_device_auth_on(ClientCertHeader::Mtls, "role", …)`. Setting the wrong header is a 401, not a fallback.
- In public-server, set that header on the individual test request, not on the `public` or `private` server directly (these should not be `mut` in tests)
- Test both success and error scenarios (especially 404 cases for non-existent resources)
- For database tests, use direct model functions instead of HTTP endpoints
- Always include `use database::ModelName;` imports in test files
- Integration tests are modules of a single `it` test binary per crate: files live in `tests/it/` and are declared with a `mod` line in `tests/it/main.rs`. Add new test files there — never as loose files directly under `tests/`, where each one links its own ~700MB binary and grinds the machine.
- Do not include `_test` suffix or prefix in test filenames
- Calling private-server endpoints in tests:
  - Endpoints are at `/api/<module>/<function>` (e.g. `/api/statuses/group_ids`)
  - Pass parameters via `.json(&serde_json::json!({"param_name": value}))`
  - For functions with no parameters, still send an empty body: `.json(&serde_json::json!({}))`

## React frontend (`private-web/`)
The React + MUI + Vite frontend lives at `/private-web/` and is embedded into the private-server binary at build time via `rust-embed`.

Local dev workflow (two terminals):
- `just watch-private-api` runs the private-server binary on `127.0.0.1:8081`. (We bind to IPv4 because Node's vite-proxy can't resolve `[::1]` literals.) `SKIP_FRONTEND_BUILD=1` is set so `cargo run` doesn't reinvoke `npm run build` on every iteration.
- `just watch-private-web` runs Vite at `:8090`, proxying `/api` to the API.

Open `http://localhost:8090/`. The Vite proxy makes the React app same-origin with the API, so no CORS plumbing is needed.

### Wire types are generated from the Rust spec

`private-web/src/api-types.ts` is **generated** from `private-web/openapi.json`,
which is in turn generated from the `#[utoipa::path]` annotations on the
private-server handlers. Both files are checked in so fresh checkouts work
without a Rust build.

Run `just gen-openapi` after changing any private-server handler's request
body, response body, security scheme, or tag. The recipe rebuilds the
`openapi-dump` binary, writes `private-web/openapi.json`, then runs
`openapi-typescript` (via `npm run gen:api-types`) to regenerate
`private-web/src/api-types.ts`. Commit both files alongside the Rust change.

`private-web/src/types.ts` is a hand-written thin re-export layer over
`api-types.ts`. Most consumer code imports from `../types`; new code can do
the same. The file also holds UI-only constants (severity ordering, label
maps, etc.) that don't belong in the wire spec.

End-to-end tests use Playwright. Run with `npm run test:e2e` from `/private-web/` (or `just test-e2e`, which also builds the binaries and uses the ramdisk Postgres). The fixture (`e2e/fixture.ts`) spawns its own private-server + Vite pair against a freshly-migrated `canopy_e2e_<random>` Postgres database per worker, so the operator does not need to keep `just watch-private-api` running. Build the binaries first with `cargo build --bin private-server --bin migrate`. Override the admin connection used to create/drop the throwaway DB with `CANOPY_E2E_ADMIN_DATABASE_URL` (default `postgres://localhost/postgres`); set `CANOPY_E2E_VERBOSE=1` to stream backend/frontend logs. The first run on a fresh checkout needs `npx playwright install chromium`.

When adding or changing a UI feature, add Playwright coverage for it in `private-web/e2e/` as part of the same change — seed state with the helpers in `e2e/seed.ts` (extend them when the feature needs new tables) and follow the existing spec patterns. Rust endpoint tests don't cover the rendered behaviour, and typecheck alone doesn't prove the feature works.

## Development Workflow
- Write or change specs in `.workhorse/specs/`; follow the spec rules in [.workhorse/rules.md](.workhorse/rules.md)
- Design docs for in-flight, not-yet-shipped work live as plans in `.workhorse/plans/`. Delete a plan once its work has shipped — the convention is a dedicated `unplan:` commit (e.g. `unplan: <what> (shipped)`); git history is the durable record, so nothing is lost by removing it.
- Always check: `just check` for basic compilation
- Run full test suite: `just test`
- Run specific tests: `just test-name <test_name>`
- Verify no compilation warnings in tests and main code, and that `cargo fmt` has been run

### Tests run on a throwaway RAM-backed Postgres by default
Each test creates and drops its own database (and runs every migration), and
nextest runs many in parallel. Against a disk-backed Postgres the resulting
`CREATE DATABASE`/`DROP DATABASE` fsync storm saturates disk I/O and can make
the whole machine unresponsive.

So `just test` (and `test-package`/`test-name`/`test-verbose`/`test-e2e`) run
through `scripts/ramdisk-pg.sh`, which spins up a disposable Postgres on tmpfs
(`/dev/shm`) with `fsync`/`synchronous_commit`/`full_page_writes` off, points
`DATABASE_URL` at it, then tears it down. Nothing touches a physical disk, so
there's no grind and runs are dramatically faster. It reuses your installed
`initdb`/`pg_ctl`, so the server version matches your system Postgres with no
container or image to manage. `just test` takes nextest args, so `just test`,
`just test -p database`, and `just test <name>` all work. Wrap other commands
with `just fast <cmd>`.

Requirements/caveats: needs the Postgres *server* tools (`initdb`/`pg_ctl`), not
just the `psql` client. On macOS there's no `/dev/shm`, so it falls back to disk
— still fast (fsync is off), just not RAM-backed unless you point
`CANOPY_TEST_PG_DIR` at a real ramdisk. Other overrides: `CANOPY_TEST_PG_PORT`,
`CANOPY_TEST_PG_ROLE`.

To run against your **system** Postgres instead (to inspect the DB afterwards,
or where `initdb` isn't available), use `just test-system [nextest args]`.

## Version Control
- If the working copy is a jujutsu repo (a `.jj` directory exists at the repo root), prefer `jj` commands over `git` for VCS operations (status, diff, log, commit/describe, etc.). The repo may be colocated with git, but `jj` is the source of truth for local work.
- If there is no `.jj` directory, use `git` as normal.

## Troubleshooting and common mistakes
- Always use just for tests, never use `cargo test`.
- Unless you've done wide-ranging changes, prefer to test specific packages with `just test-package <package_name>` instead of the full test suite.
- For frontend type checking, use `just typecheck`, not `npx tsc` at the repo root. There's no `package.json` at the root so the bare `tsc` invocation silently no-ops and you get false confidence that types are fine.
- If you're trying to import diesel in the private-server crate, stop that and put database stuff in the database crate instead.
- When files change from under you (ie when the dev changes things without telling you), assume those changes are intentional instead of reverting them.
- If you're not sure, STOP AND ASK.
