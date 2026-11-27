# A7: rig recipe and design system

Give Workhorse's loops a Canopy they can build and drive (`.workhorse/rig/`), and record the admin UI's current visual language as the design system (`.workhorse/design/design-system.md`).

## Notes

- The rig serves debug builds: the debug identity, the fake ACME authority and the seeder are compiled out of release builds, and the e2e suite runs the same way. The SPA is the production `vite build`, served by the private-server.
- Two servers per lease on one database: the base URL acts as the debug administrator; `info.headerAuthUrl` trusts `Tailscale-User-*` headers so a non-administrator (any login off the seeded allow-list) and a real operator can be exercised.
- The device API is reached through the private-server's `/public` mount. The seed step gives the first seeded device a real certificate so a tester can authenticate as it.
- `deps` compiles every dependency once per lockfile pair and each build clones that `target/`, so a build recompiles only workspace crates.
- The toolchain comes from rustup inside the rig's own home (minimal profile), since recipe steps never see the person's home.
- Each build compiles into its own `target/`. A `build.build-dir` shared across builds is not safe: Cargo takes another checkout's workspace crates as fresh while that checkout's source is still on disk.
- sccache is used only when `SCCACHE_DIR` reaches the rig; otherwise the wrapper is off, so a rig never starts an sccache server the person's own builds would then use. That the rig hides the person's toolchain and cache setup at all is Workhorse's to fix, recorded as needing a card.

## Checklist

- [x] Recipe: `rig.json`, `deps`, `build`, `seed`, `serve`, `lib.sh`, `NOTES.md`
- [x] Prove the build: `deps` and `build` through the rig manager, the commit's own build reusing the dependency build
- [x] Prove `seed` and `serve` by hand against a throwaway Postgres: the SPA renders seeded data, the header-trusting server tells admin from non-admin, the device certificate authenticates on `/public`
- [ ] Prove a full `rig lease` end to end, once the device's shared rig Postgres is running
- [x] Design system: `.workhorse/design/design-system.md` describing the current theme, shell, page anatomy, colour meanings, typography, components and copy conventions
