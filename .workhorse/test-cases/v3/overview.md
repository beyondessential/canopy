# Codegen binary bodies: test cases

What verifies that the generated client can carry a non-JSON body and query
parameters, and that doing so left every published method's call sites working.

## Compatibility with the published client

- [x] A call written against the 1.0.2 signature, holding its arguments in `String`s, compiles and reaches the envelope by conversion (verifies spec: APIC)
- [x] Such a call sends the bare path, an empty body, and no content-type header, exactly as it did before the method carried an envelope (verifies spec: API)
- [x] `cargo semver-checks` reports no semver update required against the published baseline, which is what makes the change compatible (verifies spec: API)
- [x] The methods of operations with a plain JSON body keep their published shape, taking the body as their own argument
- [ ] A consumer crate built against the published 1.0.2 and then against this crate compiles unchanged in both, across `&str`, `&String`, `&Cow<str>` and `&Box<str>` call sites — run outside the repo, so not automated here

## Carrying a body

- [x] An octet-stream body is sent as the bytes given, under `application/octet-stream`, uncompressed so what canopy digests is what the caller passed (verifies spec: APIC)
- [x] A text body is sent as the caller's text under `text/plain`, whatever characters it carries (verifies spec: APIC)
- [ ] An artifact registered through the client against a running canopy is stored with the digest of the bytes the client sent — the client crate is a cargo project of its own and reaches no test server, so this is manual

## Query parameters

- [x] A parameter a caller sets is placed in the query (verifies spec: APIC)
- [x] A parameter a caller leaves unset is absent rather than sent empty (verifies spec: APIC)
- [x] A value carrying reserved characters is percent-encoded, so it arrives as the value it was (verifies spec: APIC)
- [x] A parameter the document types as a uuid is a uuid on the method, not text (verifies spec: APIC)
- [x] A required parameter is not an option and is always placed in the query

## The generator refuses what it cannot express

- [x] A request body whose media type the client cannot send fails generation rather than being left off the method (verifies spec: APIC)
- [x] A JSON request body that is not a `$ref` fails generation
- [x] A request body declaring several media types fails generation, rather than the client choosing one
- [x] A query parameter whose schema cannot be typed fails generation
- [x] A ledger entry naming an operation the document no longer carries fails generation, rather than reshaping a published method (verifies spec: APIC)
- [x] An envelope whose name collides with a schema in the document fails generation
- [x] A path parameter named after the request argument fails generation, rather than shadowing it and building the path from the wrong value

## Shape of a method

- [x] An operation with no published method takes its envelope as a trailing argument (verifies spec: APIC)
- [x] A grandfathered operation widens its last path parameter in place, keeping its arity (verifies spec: APIC)
- [x] Regenerating from an unchanged document produces an unchanged crate (verifies spec: APIC)
