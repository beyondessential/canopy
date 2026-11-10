# Remove the public /applications listing

`GET /applications` is an alias of the unauthenticated `GET /servers` central listing, added alongside the `/applications/self` alias. Only the listing alias goes: `GET /servers` is what the Tamanu mobile app reads, and both `/self` paths already require an enrolled device certificate, so they stay.

## API version

The published `bes-canopy-api` 2.0.1 has an `applications()` method for `GET /applications`, so CI's semver gate fails on its removal unless the document's major is raised. This card raises it to 3.0.0, the way the 2.0.0 raise did: `info.version` in `crates/public-server/src/openapi.rs`, the client crate's manifest and lockfile, then regenerate. bestool-canopy calls only `servers()`, so it takes the `^3` bump with no code change.

## Steps

- [ ] Drop `list_applications` from `alias_routes()` in `crates/public-server/src/applications.rs`, keeping `application_self`; update the doc comments that describe both routes as aliased
- [ ] Replace the stale `// spec: PAC` references with the spec ids the routes now trace to
- [ ] Raise the public API version to 3.0.0 (openapi.rs, `crates/canopy-api/Cargo.toml` and its lockfile, any test pinning the version)
- [ ] `just gen-openapi && just gen-api`; commit `openapi.json` and the regenerated client
- [ ] Rework `tests/it/application_aliases.rs`: the listing test asserts `GET /applications` is not served, the self test stays
- [ ] Check `servers_list.rs`, `openapi_spec.rs` and `auth_requirements.rs` for `/applications` listing assumptions
