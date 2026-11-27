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
  `Cargo.lock` and `package-lock.json`. Each build takes a copy of that `target/` (`mode` is
  `clone`), so it recompiles only the workspace's own crates.
- `build`: `npm run build` in `private-web/`, then the debug `private-server`, `migrate` and
  `seed`. A debug build because the debug identity, the fake ACME authority and the seeder are
  compiled out of release builds. Where the file system cannot clone, `target/` is a link shared
  by every build, so the binaries are copied out to `.rig-out/`.
- `seed`: `migrate`, then `seed`, then swaps the seeder's placeholder key on the first device for
  a real certificate's, kept in `.rig-out/`.
- `serve`: the header-trusting server in the background, then the base URL's in the foreground,
  with the stand-ins the e2e fixture uses for AWS, Kubernetes and the outside world.

Needs: `cargo` (through rustup, which installs the stable toolchain into the rig's home on first
use), `node` and `npm`, `psql`, `openssl` and `curl` on the PATH the manager runs with; a Postgres
whose user in the manager's URL has `CREATEDB`.

Limits:

- Only the foreground server is watched. If the header-trusting server exits, the lease stays up
  and `info.headerAuthUrl` refuses connections.
- The background jobs (backups, monitoring, Slack, AI triage) are separate binaries and do not run.
