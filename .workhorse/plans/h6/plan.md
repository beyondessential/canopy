# Remove the public /applications listing

`GET /applications` is an alias of the unauthenticated `GET /servers` central listing, added alongside the `/applications/self` alias. Only the listing alias goes: `GET /servers` is what the Tamanu mobile app reads, and both `/self` paths already require an enrolled device certificate, so they stay.

## API version

The published `bes-canopy-api` 2.0.1 has an `applications()` method for `GET /applications`, so CI's semver gate fails on its removal unless the document's major is raised. This card raises it to 3.0.0, the way the 2.0.0 raise did: the `version` in `crates/public-server/src/openapi.rs` is the only hand edit, since `just gen-api` stamps the client crate's manifest from the document. bestool-canopy calls only `servers()`, so it takes the `^3` bump with no code change.

## Steps

- [x] Rebase onto main first: the branch predates the 2.0.1 release commit, which touched the same version declaration and generated files

### Route

- [x] `crates/public-server/src/applications.rs`: drop `list_applications` and its route from `alias_routes()`, leaving `application_self`
- [x] Rewrite the `alias_routes()` doc comment for a single aliased route, and the `routes()` comment if it still reads as pairing two
- [x] Replace both `// spec: PAC` comments: `routes()` traces to `APP` (the listing) and `DID` (self identity), `alias_routes()` to `DID`

### Version and generated files

- [x] Raise `version` in the `#[openapi(info(...))]` of `crates/public-server/src/openapi.rs` to `3.0.0`
- [x] Run `just gen-openapi && just gen-api`; commit `crates/public-server/openapi.json`, `private-web/openapi.json`/`api-types.ts` if they move, `crates/canopy-api/Cargo.toml`, its `Cargo.lock` and `src/generated.rs`
- [x] Confirm `just check-generated` and `just semver-checks` pass

### Tests

- [x] `crates/public-server/tests/it/application_aliases.rs`: replace `the_listing_reads_the_same_under_both_names` with a test that `GET /applications` is not served (verifies APP's only-unauthenticated-listing criterion); update the module doc, which describes both routes as aliased
- [x] `crates/public-server/tests/it/servers_list.rs`: fix the header comment that calls these `GET /applications` tests
- [x] Run `just test -p public-server application_aliases` and `just test -p public-server servers_list`, then `just check-api-crate`
