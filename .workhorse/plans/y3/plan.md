# Stop CI Rust caches evicting each other

## Decision: ship `save-if: main` only, then measure

The load-bearing fix is stopping PRs from saving caches. With `add-job-id-key`
and matrix jobs sharing one `github.job` id, main's steady-state footprint is
roughly one cache each for `test`, `e2e`, `clippy`, `generated`, `semver`,
`api-crate` (~6 keys). Once PRs stop saving, that sits under 10 GB on its own,
so grouping is headroom, not the fix. Ship the minimal change, watch a busy day,
and only revisit grouping if main still crowds 10 GB.

## Work

- [ ] Add `save-if: ${{ github.ref == 'refs/heads/main' }}` to all six
      `Swatinem/rust-cache@v2` steps in `.github/workflows/ci.yml`:
      `test` (~35), `clippy` (~70), `e2e` (~113), `generated` (~190),
      `semver` (~217), `api-crate` (~235). PRs restore main's caches, only main
      saves.
- [ ] After merge, watch a busy day: confirm main's caches survive, a PR's
      Generated files job restores main's cache, and total usage stays < 10 GB.

## Dropped from the original card

- **`SKIP_FRONTEND_BUILD` on `generated`** — already handled. The justfile
  globally `export SKIP_FRONTEND_BUILD := "1"` (justfile:15), and `generated`
  runs `just check-generated`, so `private-server/build.rs` already skips the
  `npm ci && npm run build`. The 12-min cold run is fully explained by
  "No cache found", not by npm. Adding the env var would be redundant.

## Deferred: `shared-key` grouping (only if headroom is still tight)

Analysis for when/if we come back to it:

- **Two disjoint build universes, not one.** `crates/canopy-api` is its own
  workspace with its own `Cargo.lock` and `target/` (root `Cargo.toml:25`
  excludes it). Root-workspace jobs (`test`, `e2e`, `generated`, `clippy`)
  and canopy-api jobs (`semver`, `api-crate`) build into different target dirs.
  A single blanket `shared-key` across both is harmful — each universe restores
  the other's unrelated `target/`. rust-cache's default `. -> ./target` likely
  means `semver`/`api-crate` don't cache compiled canopy-api artifacts at all
  today (only `~/.cargo`), so they bloat little and gain little from grouping.
- **Grouping trades hit quality for cache count.** Designate one saver, others
  `save-if: false` (no first-to-finish race). Cache content is exactly the
  saver's `target/`. `test` builds test-profile artifacts (dev-deps,
  `cfg(test)`); `e2e`/`generated` build plain debug `private-server`. Cross-
  profile restores recompile the differing artifacts.
- **Preferred grouping if we do it:** group only `e2e` + `generated` under one
  key with `e2e` saving (its `cargo build --bin private-server --bin migrate`
  is the closest superset of what `generated`'s openapi-dump needs). Leave
  `test` on its own key — it is the heaviest job and wants a perfect hit, and
  on PRs it already restores main's `test` cache once eviction stops. That is
  ~4 main caches with no degraded hits. Before committing, measure: fill a
  target dir with `e2e`'s build, run the dump build, count recompiles.
