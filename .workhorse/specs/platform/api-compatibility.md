---
id: API
---

# Public API compatibility

Canopy's public API is consumed by agents in the field, on their own upgrade schedule.
An agent that was working before a Canopy deploy is working after it, so a change to the public API is compatible unless breaking it has been coordinated.

Coordination is what makes a break permissible, and it is a decision taken deliberately rather than a consequence noticed later.
Every change that is not coordinated is held to the definition below.

## The definition

Canopy publishes a Rust client generated from its public OpenAPI document (see [APIC](api-client-crate.md)).
A change to the public API is compatible when the crate generated from the document after the change is not a semver-breaking change against the crate generated from it before, as `cargo-semver-checks` judges it.

The definition is mechanical on purpose.
"Keep the wire compatible" is a judgement each author makes differently; regenerating the crate and comparing it is a check that either passes or fails, so a break is found before it ships rather than reported from the field.

The public API is what the definition governs, because that is what the agent-facing crate is generated from.
The private API is the admin interface's own and ships in the same binary as the interface that calls it, so it carries no such obligation.

## What the definition makes a break

Removing a path, an operation, a schema, a property, or a variant of an enumeration is a break, since each removes something a generated client declares.

Making a request property required when it was optional is a break, because a client that never sent it stops being accepted.

Changing a property's type is a break, including widening it to accept null: the generated field changes shape even though what the wire accepts only grows.

Adding a path, an operation, a schema, or an optional property is compatible.

## Renaming a field

A field is renamed by adding the new name and keeping the old one, never by replacing it.

A response carries both names for as long as the old one is supported, so a consumer reading either finds what it needs.
A request accepts either name, and neither is required on its own.

A request that names the same thing twice is refused rather than resolved by preference.
A client that disagrees with itself about what it is describing has not been understood, and preferring one of the two values would record a fact nobody asserted.

## Surfaces the definition does not reach

The keys Canopy returns inside a map are not part of the schema, so removing a key a consumer reads breaks it without the definition noticing.
A map's value type is generated into the client and judged with the rest of it, so the key set alone is what the definition leaves unreached.
The reserved tags Canopy returns to a reporter are such a surface (see [STA](../public-server/statuses.md)).

A key that has been served is kept and kept populated, on the same terms as a schema property, because a consumer reading it cannot tell a removed key from an absent value.
Where the key's meaning has moved, its value is derived from wherever that meaning now lives rather than dropped.

A parameter widened to accept anything convertible into a type is judged on the signature the crate declares, not on what a call site can still pass, so a widening that narrows what the parameter accepts satisfies the definition and breaks consumers anyway.
Deref coercion does not reach through a generic: a conversion declared from `&str` alone takes a literal and refuses the `&String` a call site more often holds.
A widened parameter therefore accepts everything the type it replaced accepted, which is what makes the widening compatible, and the definition does not ask for it (see [APIC](api-client-crate.md)).
