---
id: APIC
---

# Public API client crate

Canopy publishes `bes-canopy-api`, a Rust client for its public API, generated from the OpenAPI document Canopy holds for that API.
A consumer reaching Canopy from Rust depends on the published crate rather than describing the API itself, so the wire types it works with are the ones Canopy declares.

## Generated in the Canopy repository

The document the crate is generated from is held in Canopy's own source, so generation reads only the repository.

The generated source is committed alongside that document, so a change to the client's surface appears in the change that causes it and is reviewable there.
Regenerating from an unchanged document produces an unchanged crate.

Everything generated is parsed and formatted before it is written, whether it was derived from a schema or written by the generator itself.
One place decides how a generated struct is constructed, so the builder and the openness that make a schema's growth compatible reach every generated type; and source the generator got wrong fails generation rather than a consumer's build.

## One version for the document and the crate

The document and the crate carry the same version, because the crate is derived from the document and its surface moves only when the document or the generator moves.
The document declares the version and the crate takes it, so the two cannot drift apart.

The version describes the crate's surface.
A change in what the generator emits raises it as a change to the document would, even where the API the document describes is untouched.

The version is not an input to generation.
A release generates the crate, judges the change against the published crate as [API](api-compatibility.md) defines, and then records the resulting version, so the version is settled after the change it describes is final.

A compatible change raises the minor or patch version.
A break raises the major version, and that raise is where the coordination [API](api-compatibility.md) requires is recorded.

## Every operation is typed

Each operation in the document has a method on the client, taking and returning types generated from the document's schemas.
Method names are derived from the operation's path, with the verb distinguishing the methods of a path served by more than one verb, so a consumer's call sites depend on the path rather than on the order operations were generated in.

An operation is reached through its generated types rather than through an untyped JSON body.
A schema the generator cannot express is a defect in the document or in the generator, resolved there rather than by degrading that operation to untyped JSON.
A request body or a parameter the generator cannot express is the same defect and fails generation, rather than being left off the method: a method that drops what its operation requires cannot do the thing it is named for.
So is an operation whose identifier has no Rust form, since the type generated for it would be named after one.
A parameter reached through a reference, or carried somewhere the client does not send, is refused on the same terms, and a parameter a path item declares for every operation under it reaches each of those operations.

A schema that is a typed object carrying arbitrary further keys generates a struct with its declared fields and a map holding the rest, so a consumer both sends and reads those further keys.
A schema that is a map with a declared value type generates a map of that type.

## What an operation's method takes

An operation's path parameters are the arguments of its method, and everything else it carries — its request body, and its query parameters — travels in one request type generated for that operation.
A query parameter is a field of that type rather than an argument of its own, so adding one later adds an optional property to a generated struct, which is compatible, rather than changing a method's arity, which is not.
That type is constructed the way every generated struct is, naming only the fields a caller sets.

A parameter is typed as the document describes it rather than as the text it becomes on the wire.
A value a caller supplies is encoded where it is placed in the query, so a value carrying reserved characters reaches Canopy as the value it was.
A parameter a caller leaves unset is absent from the request rather than sent empty, because Canopy tells an absent parameter from an empty one.

A value a caller places in the path is refused when it would decide which endpoint is called, rather than encoded.
A value carrying a URI delimiter reroutes the request or puts parameters of its own ahead of the ones the caller asked for; a value that is a dot segment resolves a segment away without carrying a delimiter at all; a value carrying an escape reaches a server that decodes before it resolves; and an empty value collapses a segment.
Encoding such a value instead would change what every call already written puts on the wire.

A request body the document declares as something other than JSON is carried as the text or the bytes it is, and sent under the media type the document names.
It is sent as it stands rather than compressed, so what Canopy digests is what the caller passed.
Whether a caller may leave that body unset is the document's to say, except on a method whose published signature has already settled it by being unable to send one.

A method published before its operation's body could be expressed keeps its name and its arity, widening its last path parameter to accept that operation's request type rather than gaining an argument.
The widened parameter accepts everything the parameter it replaced accepted, so a call written against the published signature compiles unchanged and sends what it always sent, while a caller that needs the body supplies the request type instead (see [API](api-compatibility.md)).
The generator holds the ledger of which methods these are.
An entry naming an operation the document no longer carries, or one that no longer carries a request to widen into, fails generation rather than reshaping the method that entry exists to hold still.
Such a method carries nothing required beyond that path value, because its published call sites supply nothing else to put there.
Whether a method carries a body at all is settled where it is generated rather than inferred from the size of what it carries, so a body that is present and empty still declares what it is.

## The consumer supplies the transport

The crate leaves how a request reaches Canopy to its consumer, and depends on no particular HTTP client.
Every generated method works over whichever transport the consumer supplies.

A transport receives a request whose target is a path, and resolves the host, the scheme, and the authentication itself.
It returns Canopy's response as given, unsuccessful statuses included, because endpoints give particular statuses a meaning only the client can read.
A failure to obtain any response is reported as distinct from a response that reports failure.

The client turns an unsuccessful status into an error carrying that status and the body, so a consumer can branch on the status an endpoint documents.

## What the generated types carry

A field holding a credential secret is readable but does not appear in debug output, so a consumer logging a response does not disclose it.
A field the document describes as a timestamp is typed as a timestamp rather than as text.

A schema gaining a field leaves construction of that schema's type working for a consumer that does not set it.

The crate records the document it was generated from, so a document that changed without the version moving with it can be told from one that did not.
