# Canopy Development Commands
# Default database URL (can be overridden)

export DATABASE_URL := env('DATABASE_URL', 'postgres://localhost/canopy')
export RO_DATABASE_URL := env('RO_DATABASE_URL', 'postgres://localhost/canopy')

# ...for development

export SERVER_VERSIONS_SECRET := "test"
export PUBLIC_URL := "http://localhost:8080"

# Skip the npm install + npm run build that private-server's build.rs runs by default.
# Set this in dev recipes — Vite serves the frontend directly there, and we don't
# need the binary to embed dist/ for cargo check / cargo run / cargo test.
export SKIP_FRONTEND_BUILD := "1"

# Show available commands
default:
    @just --list

# Check if the project compiles
check:
    scripts/contain.sh cargo check

# Build the project Docker image
build-image:
    docker build -t canopy .

# Run the public server and reload on change
watch-public:
    watchexec -I -w crates -- cargo run --bin public-server

# Rebuild the private-server binary on source change (pair with watch-private-api)
watch-private-build:
    watchexec -I -w crates -- cargo build --bin private-server

# Run the private server's HTTP API, bound to 127.0.0.1:8081, for the private-web Vite frontend.
# Watches the built binary so it restarts when watch-private-build produces a fresh artefact.
# CLIENT_IP_SOURCE is overridden to ConnectInfo because the prod default
# (RightmostXForwardedFor, for behind the Tailscale K8s Operator's ingress)
# would have the axum-client-ip middleware reject every request — locally
# there's no proxy in front to set X-Forwarded-For.
watch-private-api:
    CLIENT_IP_SOURCE=ConnectInfo BIND_ADDRESS=127.0.0.1:8081 watchexec -I -W target/debug -f private-server -- target/debug/private-server

# Run the private-web React frontend dev server (Vite proxy expects watch-private-api)
watch-private-web:
    cd private-web && npm run dev

# Run all tests. Uses a throwaway RAM-backed Postgres (tmpfs + fsync off) via
# scripts/ramdisk-pg.sh so the per-test CREATE/DROP DATABASE churn never hits
# disk — fast, no I/O grind. The whole run (compile, postgres, tests) sits in
# a resource-limited cgroup via scripts/contain.sh so it can't freeze the
# machine. Args pass straight to nextest, so `just test`, `just test -p
# database`, and `just test some_name` all work. Use test-system to run
# against $DATABASE_URL instead.
test *args:
    scripts/contain.sh scripts/ramdisk-pg.sh cargo nextest run --no-fail-fast {{ args }}

# Run tests for a specific package (RAM-backed; see `test`)
test-package package:
    scripts/contain.sh scripts/ramdisk-pg.sh cargo nextest run --no-fail-fast -p {{ package }}

# Run a specific test (RAM-backed; see `test`)
test-name name:
    scripts/contain.sh scripts/ramdisk-pg.sh cargo nextest run --no-fail-fast {{ name }}

# Run tests with no capture (show output) (RAM-backed; see `test`)
test-verbose:
    scripts/contain.sh scripts/ramdisk-pg.sh cargo nextest run --no-fail-fast --no-capture

# Run tests against your system Postgres ($DATABASE_URL) rather than the
# throwaway RAM-backed one — e.g. to inspect the DB afterwards, or where
# initdb/pg_ctl aren't available. Args pass through to nextest.
# `just test-system`, `just test-system -p database`, `just test-system some_name`.
test-system *args:
    DATABASE_URL={{ DATABASE_URL }} scripts/contain.sh cargo nextest run --no-fail-fast {{ args }}

# Run any command against the throwaway RAM-backed Postgres (escape hatch for
# things the test recipes don't cover).
fast +cmd:
    scripts/contain.sh scripts/ramdisk-pg.sh {{ cmd }}

# Run the private-web Playwright end-to-end suite. Builds the
# private-server + migrate binaries first (the e2e fixture spawns its
# own server/Vite per worker — no `just watch-*` needed). Runs against the
# throwaway RAM-backed Postgres; the fixture creates its per-worker databases
# on the cluster the wrapper points CANOPY_E2E_ADMIN_DATABASE_URL at.
test-e2e:
    scripts/contain.sh cargo build --bin private-server --bin migrate
    cd private-web && {{ justfile_directory() }}/scripts/contain.sh {{ justfile_directory() }}/scripts/ramdisk-pg.sh npm run test:e2e

# Frontend unit tests (vitest). No browser or database needed — this is the
# fast lane for pure logic in private-web/src/lib, such as the mirrors of Rust
# evaluators that must not drift from their source of truth.
test-web:
    cd private-web && npm run test

# Same as `test-e2e` but launches Playwright's interactive UI runner.
# Useful for stepping through failures and inspecting traces.
test-e2e-ui:
    cargo build --bin private-server --bin migrate
    cd private-web && npx playwright test --ui

# Typecheck the private-web React frontend. Running `npx tsc` at the
# repo root silently no-ops (no package.json), so always go through
# this recipe. `-b` is load-bearing too: the root tsconfig is
# references-only with `"files": []`, so a bare `tsc --noEmit` checks
# nothing and always exits 0; build mode follows the references and
# matches what CI's `npm run build` runs.
typecheck:
    cd private-web && npx tsc -b

# Run migrations
migrate:
    DATABASE_URL={{ DATABASE_URL }} diesel migration run
    cargo fmt

# Seed the local dev database with representative fake data (LOCAL DEV ONLY: truncates+repopulates app tables, run `just migrate` first, refuses prod-looking URLs)
seed:
    DATABASE_URL={{ DATABASE_URL }} cargo run --bin seed

# Create a new migration
migration name:
    DATABASE_URL={{ DATABASE_URL }} diesel migration generate {{ name }}

# Redo the last migration (down then up)
migrate-redo:
    DATABASE_URL={{ DATABASE_URL }} diesel migration redo
    cargo fmt

# Revert the last migration
migrate-revert:
    DATABASE_URL={{ DATABASE_URL }} diesel migration revert
    cargo fmt

# Format code
fmt:
    cargo fmt

# Check formatting without making changes
fmt-check:
    cargo fmt --check

# Run clippy lints
lint:
    scripts/contain.sh cargo clippy --all-features --all-targets

# Fix clippy warnings automatically where possible
lint-fix:
    cargo clippy --all-features --all-targets --fix --allow-dirty --allow-staged

# Generate identity certificate for API authentication
identity:
    cargo run --bin identity

# Regenerate the OpenAPI specs (private-server → private-web for TS codegen,
# public-server → its crate directory for external visibility). Run this after
# any change to a handler's request/response shape, security scheme, or tag.
gen-openapi:
    cargo run --quiet --bin private-openapi-dump > private-web/openapi.json
    cargo run --quiet --bin public-openapi-dump > crates/public-server/openapi.json
    cd private-web && npm run gen:api-types

# Regenerate the published API client (crates/canopy-api) from the public
# server's OpenAPI document. Run this after `gen-openapi` whenever the public
# API changes; the generated source is committed alongside the document.
gen-api:
    cargo run --quiet --locked -p canopy-api-codegen
    cargo metadata --manifest-path crates/canopy-api/Cargo.toml --format-version 1 > /dev/null

# Check the published API client depends on no HTTP client: how a request
# reaches canopy is its consumer's to decide, so the crate carries the wire
# types and the call plumbing but no transport.
check-api-deps:
    #!/usr/bin/env bash
    set -euo pipefail
    deps=$(cargo tree --manifest-path crates/canopy-api/Cargo.toml --edges normal --prefix none | awk '{print $1}' | sort -u)
    for forbidden in reqwest hyper h2 rustls native-tls curl isahc ureq; do
        if grep -qx "$forbidden" <<<"$deps"; then
            echo "bes-canopy-api must not depend on $forbidden: the transport is the consumer's" >&2
            exit 1
        fi
    done

# Check the committed OpenAPI document and API client are what the current code
# generates. CI runs this; a diff means `just gen-openapi && just gen-api`
# wasn't run (or wasn't committed).
check-generated:
    #!/usr/bin/env bash
    set -euo pipefail
    # --locked so what is generated depends only on committed inputs: an
    # unlocked resolve could pick a newer codegen dependency than Cargo.lock
    # records and emit different source here than it does on a dev machine.
    generated=(crates/public-server/openapi.json private-web/openapi.json private-web/src/safety-modes.ts crates/canopy-api/src/generated.rs crates/canopy-api/Cargo.toml crates/canopy-api/Cargo.lock)
    cargo run --quiet --locked --bin public-openapi-dump > crates/public-server/openapi.json
    cargo run --quiet --locked --bin private-openapi-dump > private-web/openapi.json
    # The grade map the interface reads. Node builtins only, so this needs no
    # npm install: a stale map would block or offer controls against grades the
    # server no longer enforces.
    node private-web/scripts/gen-safety-modes.mjs
    cargo run --quiet --locked -p canopy-api-codegen
    cargo metadata --manifest-path crates/canopy-api/Cargo.toml --format-version 1 > /dev/null
    # Against HEAD rather than the index: a staged-but-uncommitted regeneration
    # would otherwise make a stale commit look current.
    if ! git diff --quiet HEAD -- "${generated[@]}"; then
        echo "generated files are out of date; run 'just gen-openapi && just gen-api' and commit the result" >&2
        git --no-pager diff --stat HEAD -- "${generated[@]}" >&2
        exit 1
    fi

# Check the published API client is not a semver-breaking change against what is
# on crates.io, which is how `.workhorse/specs/platform/api-compatibility.md`
# defines a compatible change to the public API. The version in the crate's
# manifest (stamped from the document's info.version) is what decides how much
# change is permitted: cargo-semver-checks reads the bump against the published
# baseline, so a coordinated break passes once info.version raises the major.
semver-checks:
    cargo semver-checks --manifest-path crates/canopy-api/Cargo.toml --package bes-canopy-api

# Run the workspace's own checks over the published API client, which is a cargo
# project of its own (see release-plz.toml) and so is invisible to every recipe
# above: `cargo fmt`, `clippy`, `check` and `nextest` at the root do not reach it.
check-api-crate:
    cargo fmt --check --manifest-path crates/canopy-api/Cargo.toml
    cargo clippy --manifest-path crates/canopy-api/Cargo.toml --all-features --all-targets -- -D warnings
    cargo test --manifest-path crates/canopy-api/Cargo.toml

# Clean build artifacts
clean:
    cargo clean

# Build server binaries for a specific target (release mode), with embedded private-web frontend
build-servers-release target:
    SKIP_FRONTEND_BUILD= cargo build --locked --target {{ target }} --release --bins

# Install development dependencies
install-deps:
    cargo binstall -y cargo-binstall || cargo install cargo-binstall
    cargo binstall -y cargo-nextest watchexec-cli diesel_cli

# Download database from Kubernetes
download-db dbname namespace="canopy-dev" pod="meta-db-1" output="app.dump":
    dropdb {{ dbname }} || true
    createdb {{ dbname }} || true
    kubectl exec -n {{ namespace }} {{ pod }} -c postgres -- pg_dump -Fc -d app > {{ output }}
    pg_restore --no-owner --role=$USER -d {{ dbname }} --verbose < {{ output }}

# Development cycle: format, lint, test
dev: fmt lint test
