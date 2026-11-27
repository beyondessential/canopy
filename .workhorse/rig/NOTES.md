# Canopy rig

A lease is two debug-built private-servers on one database seeded by the dev seeder, serving the
production build of the SPA, on 127.0.0.1. The device API is the private-server's nested
`/public` mount, so there is no separate public-server.

- The server on the lease's base URL acts as the debug identity, `admin@localhost`, an
  administrator with a danger-mode session, on every request.
- The server on `info.headerAuthUrl` trusts the `Tailscale-User-*` request headers
  (`CANOPY_TRUST_TAILSCALE_HEADERS`), so a tester chooses who they are per request: a login on the
  seeded allow-list is an administrator, any other is not.
- `info.deviceCertificateHeader` authenticates a request to `/public/...` as `info.deviceId`, the
  device behind the Suva Facility application.

Entry points:

- `deps`: `npm ci` in `private-web/`, and a debug `cargo build` of the three binaries, once per
  `Cargo.lock`, `package-lock.json`, `rust-toolchain.toml` and `lib.sh` (which sets up the toolchain
  and cache, and so decides the paths a dependency build is fingerprinted by). Each build takes a copy of that `target/` (`mode` is
  `clone`), so it recompiles only the workspace's own crates.
- `build`: `npm run build` in `private-web/`, then the debug `private-server`, `migrate` and
  `seed`. A debug build because the debug identity, the fake ACME authority and the seeder are
  compiled out of release builds. Where the file system cannot clone, the manager links `target/`
  to the dependency build instead, and the build replaces the link with a copy of its own: with a
  shared `target/`, Cargo takes another commit's workspace crates as fresh while that commit's
  source is still on disk.
- `seed`: `migrate`, then `seed`, then swaps the seeder's placeholder key on the first device for
  a real certificate's, kept in `.rig-out/`.
- `serve`: the header-trusting server in the background, then the base URL's in the foreground,
  with the stand-ins for AWS, Kubernetes and the outside world read from `scripts/fake-deps.env`,
  the same file the e2e fixture reads.

Needs: `cargo` (through rustup), `node` and `npm`, `psql`, `openssl` and `curl` on the PATH the
manager runs with; a Postgres whose user in the manager's URL has `CREATEDB`.

Rust tooling is the person's: the recipe takes `CARGO_HOME`, `RUSTUP_HOME`, the compiler wrapper
and sccache's variables from their environment (`env.fromPerson`), and where they are unset points
them at `.cargo`, `.rustup` and sccache's cache under the person's home (`env.personHome`). So a rig
build uses their cargo config, registry cache, toolchains and compiler cache, and an sccache
server the rig needs is started in a systemd scope of its own, so it outlives the step and is not charged to it, and caches where the person's own builds do. Without a rustup under the person's home,
rustup installs a minimal stable toolchain into the rig's home on first use. Incremental
compilation is off in every step, since a rig build is built once and sccache cannot cache it.

A machine-wide jobserver (steve, or a make that set one up) reaches the rig through `MAKEFLAGS`,
which Cargo reads, so rig builds take their jobs from the same pool as everything else on the
machine.

Limits:

- Only the foreground server is watched. If the header-trusting server exits, the lease stays up
  and `info.headerAuthUrl` refuses connections.
- The background jobs (backups, monitoring, Slack, AI triage) are separate binaries and do not run.
