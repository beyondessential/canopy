# Codegen binary bodies, without breaking the client

`feat/codegen-binary-bodies` (490ff8ef) teaches the client generator to express
`application/octet-stream` request bodies and query parameters. The capability is
wanted; the shape it arrives in is not, and it cannot land as pushed.

## What is actually broken today

Three published operations carry a request body the generated client has never
sent, because the generator silently dropped every body shape it could not express:

| Operation | Method in 1.0.2 | Dropped |
| --- | --- | --- |
| `POST /artifacts/{version}/{artifact_type}/{platform}` | `artifacts` | `text/plain` download URL, `group`, `digest` |
| `POST /artifacts/groups/{group}/{version}/{artifact_type}/{platform}` | `artifacts_groups` | `application/octet-stream` bytes, `run` |
| `POST /versions/{version}` | `post_versions` | `text/plain` body |

The wire contract is fine and is in live use: the artifacts endpoint is driven by
at least two clients posting a plain-text body straight from curl. Only the Rust
client is deficient, so nothing about the document, the routes, or the handlers
needs to move.

The branch also misses the `text/plain` case entirely, handling only octet-stream
and JSON, so `artifacts` and `post_versions` would still drop their bodies.

## Why the branch's shape is wrong

It renders query parameters as positional arguments (`group: Option<&str>`,
`digest: Option<&str>`). That is the one shape that cannot grow: the day any
endpoint gains a parameter, every method on it changes arity and breaks again.

The generator already argues against this itself. `relax_construction` puts
`#[derive(::bon::Builder)]` and `#[non_exhaustive]` on all 42 generated structs,
reasoning that a struct built with a literal "would break the moment canopy adds
a field", and that the builder "is also what makes adding an optional property a
compatible change". Parameters deserve the same treatment as properties.

Two smaller defects fall out of the same hole: `group` and `run` are typed `&str`
when the document says `format: uuid`, and nothing stops a future inexpressible
body being dropped in silence all over again.

## The approach: widen in place, do not add

An argument cannot be *added* to a published method — arity is a break whatever
the type. But an existing argument can be *widened*, and old call sites survive.
Each of the three methods keeps its name and its arity; its last positional
parameter becomes `impl Into<XRequest>`, where `XRequest` is a generated envelope
carrying that parameter, the body, and the query parameters.

```rust
#[derive(::bon::Builder)]
#[builder(on(String, into), on(::bytes::Bytes, into))]
#[non_exhaustive]
pub struct GroupArtifactRequest {
    pub platform: String,
    pub body: Option<::bytes::Bytes>,
    pub run: Option<::uuid::Uuid>,
}

impl<T: AsRef<str> + ?Sized> From<&T> for GroupArtifactRequest { /* … */ }
```

Adding a query parameter later is then adding an optional field to a
`non_exhaustive` bon struct, which is compatible: the client's growth rules end
up mirroring the document's own rule for adding an optional property.

`body` is `Option` because `From<&T>` must be able to build a request without
one. An existing call site therefore still compiles *and still sends no body* —
behaviour is bit-identical to 1.0.2, rather than silently starting to send
something.

### The conversion impl is load-bearing

There must be exactly one `impl<T: AsRef<str> + ?Sized> From<&T>` per envelope,
and never a hand-written `impl From<&str>`.

Deref coercion does not flow through generics. With `From<&str>` alone,
`c.artifacts(&version, &atype, &platform)` where those are `String`s fails to
compile, which is the commonest call shape there is. The `AsRef<str>` blanket
covers `&str`, `&String`, `&Cow<str>`, `&Box<str>` and the envelope itself in one
impl.

The two cannot be combined: a `Deref<Target = str>` blanket alongside
`impl From<&str>` is rejected for coherence ("upstream crates may add a new impl
of `Deref` for `str`"). `AsRef<str>` is the one that works, because `str` implements
it. The residue is a consumer newtype that derefs to `str` without implementing
`AsRef<str>`.

### Verified, not assumed

Against the published 1.0.2 baseline, with the widened `artifacts_groups`:

- `cargo semver-checks` reports "no semver update required" at a *patch* bump
  (223 checks, 223 pass); a pristine control reports the same, so the harness is sound
- a simulated 1.0.2 consumer compiles unchanged, passing both string literals and
  `&String`
- the new capability (bytes plus `run`) is expressible through the builder

## New operations take the clean shape

The absorbed-last-parameter form exists only to preserve three published
signatures. An operation with no published baseline takes all path parameters
positionally and one trailing envelope argument, and gets an envelope even when
it has a single thing to carry, so it can gain parameters later without ever
needing this treatment.

Which three methods are grandfathered is an explicit list in the generator: the
compatibility ledger, small and greppable, with `cargo-semver-checks` as backstop.

## The guardrail

The root defect is that the generator degraded operations in silence. It should
hard-error on any request body or parameter shape it cannot faithfully express,
which is what APIC already requires: "A schema the generator cannot express is a
defect in the document or in the generator, resolved there rather than by
degrading that operation to untyped JSON." A lossy method then cannot ship again.

## Spec changes

`.workhorse/specs/platform/api-compatibility.md` gains a case under "Surfaces the
definition does not reach". A parameter widened to `impl Into<T>` whose conversion
set is narrower than the coercions the old concrete type allowed breaks consumers
while passing every check: the `From<&str>`-only version above provably breaks
`&String` callers and still passes 223 of 223. The check cannot see call sites, so
what keeps a widening safe is the blanket conversion, not the check.

`.workhorse/specs/platform/api-client-crate.md` gains the envelope and its growth
rule under "Every operation is typed", and the generator's obligation to fail
rather than drop a shape it cannot express.

## Build steps

- [ ] Teach the generator `text/plain` bodies (`&str`) alongside octet-stream (`Bytes`)
- [ ] Type query parameters from the document's schema rather than as `&str`
- [ ] Generate a per-operation envelope carrying body and query parameters, with the bon/`non_exhaustive` treatment and the single `AsRef<str>` blanket conversion
- [ ] Widen the three grandfathered methods' last parameter, driven by an explicit compat list
- [ ] Emit the clean trailing-envelope shape for every other operation
- [ ] Hard-error on any body or parameter shape the generator cannot express
- [ ] `just gen-openapi && just gen-api`, commit the regenerated document and client
- [ ] Confirm `just semver-checks` passes against the published baseline, and `just check-generated` is clean
- [ ] Update the two platform specs above
