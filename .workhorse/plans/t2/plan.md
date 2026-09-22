# Gate the administrative surface behind safety modes

Implementation notes for [SAFE](../../specs/private-server/safety-modes.md) and the danger permission added to [ADM](../../specs/private-server/admin-access.md).

The bulk of the work is grading a little over 200 handlers across 27 modules. Everything else is the machinery that makes the grading mean something.

## Declaring the grade

The grade goes in each module's `routes()` table, through the vendored `routes!` macro in `vendor/canopy-utoipa-axum`: `routes!(danger: delete)` and so on.
Omitting it fails to match the macro arm, so an ungraded handler is a compile error with no extra machinery to build.

One declaration does three things: registers the route as it does today, injects the OpenAPI operation extension, and attaches the grade to the route for enforcement.
Nothing is written twice, so nothing can drift.

The cost is that the grade sits away from the handler body.
The gain is that each module's route table becomes a grading table a reviewer reads in one screen, which is what makes 200-odd judgements reviewable at all: `backups.rs` is 3340 lines and 28 handlers, and its grading would otherwise be scattered through them.

Enforcement is a small layer putting the grade into request extensions, plus one middleware that reads the grade, the session, and the identity, and decides.

## Two boundaries, two mechanisms

`TailscaleAdmin` stays exactly as it is: resolves the identity, checks administrator status, upserts the cached user for avatars.
The mode and danger checks are a separate layer.
Identity resolves twice per graded request as a result, which is the price of leaving a working administrator path untouched on an already-large card.

The danger permission is re-resolved on every danger-graded request, against the allowlist and policy, exactly as the administrator check is today.
The session records a mode; it never records a permission and never grants anything by itself.

## The session

A session is a row: its identifier, the login it belongs to, its mode, when the raise expires, and when it was last seen.
The server mints one when a client connects, so a session always exists and a raise modifies the one already there.
The identifier travels as a request header, added centrally in `callApi`.

A signed token carrying login, mode, and expiry would need no storage and would survive a restart for free.
It is rejected because X3 reads the live sessions to show who is raised and where, and a token held only by its client cannot be enumerated; and because a raise has to be able to end before its expiry, which a token cannot.

Two clocks.
A raise expires ten minutes after it is made, and the server honours it for eleven, the extra minute being slack for a request in flight rather than a window anyone is offered.
A session's own liveness is separate and idle-based, since most sessions never raise.
A request updates its session's last-seen, and a periodic sweep retires sessions not seen for a while, alongside the domain and certificate sweeps already in the jobs crate.

The session identifier has to be reachable from `callApi`, which is a bare function called from outside React, so the provider publishes it to a module-level value that `callApi` reads rather than threading it through every call site.

## Carrying the grade to the client

The grade lands in `openapi.json` as an operation extension; utoipa 5.5's `Operation` has an `extensions` map for `x-` keys.

`openapi-typescript` does not surface `x-` extensions in generated types, so the grade cannot arrive as a TypeScript type.
`just gen-openapi` gains a step reading the emitted `openapi.json` and writing a generated module mapping each module and function to its grade.
That is the shape the interface wants anyway: `callApi(module, fn)` is a runtime pair, and a component choosing a stripe needs a runtime value.
The generated file is committed alongside `openapi.json` and `api-types.ts`.

## The debug identity, and 322 tests

The largest implementation risk on this card, and it is not in the feature.

Both tailnet extractors short-circuit in debug builds to a fixed `admin@localhost`, so every debug build is an administrator for free.
The private-server suite has 322 test functions across 108 endpoint paths, POSTing straight at handlers with no session of any kind.
A mode layer refusing unraised requests breaks nearly all of them at once.

So the debug shortcut extends to the new boundary: with the dev identity in play, a request is treated as holding a danger-mode session and the danger permission, by skipping the checks rather than seeding anything.
This mirrors what the shortcut already does for administrator status and is compiled out of release by the same guard, so nothing can reach it in production.
Existing tests stay untouched.

Tests exercising the boundary opt into the real path with `CANOPY_TRUST_TAILSCALE_HEADERS=1` and drive sessions explicitly, which is the pattern `admin_auth.rs` sets and which card S2 built for exactly this.

## Naming collisions already in the tree

`OperatorPresence` and `operator_presence.rs` are the monitored-server feature, nothing to do with operators using canopy.
`ActionButton` is the icon button that reveals its label on hover, so the graded control wrappers need their own name.

## Departing from seedling

The concept comes from seedling, but seedling's safety mode is advisory: its server records and broadcasts the mode each client reports and gates nothing on it.
Canopy enforces instead, because the danger permission has to be real and a mode the server already holds is the natural place to hang it.

The visual convention is taken from seedling deliberately and unchanged, so an operator who knows one interface knows the other.
Seedling's elevation window is 9m59s so a minute-rounded countdown opens at "10m"; canopy's ten against eleven is a different device for a different reason, since seedling's mode gates nothing and so needs no slack.

## Build steps

- [x] Migration adding the danger column to `admins`, made with `just migration`
- [x] Migration adding the sessions table, and its model in the database crate
- [x] Grade type in the shared types crate, ordered so the ladder is a comparison — `commons_types::safety::SafetyMode`
- [x] Grant resolution returns both permission sets where it returns one today — `resolve_permissions`, `has_danger_by_policy`, `Admin::check_danger`/`set_danger`, `TailscaleUser::has_danger`
- [ ] Grade argument and extension injection in the vendored `routes!` macro
- [ ] Enforcement layer and middleware, including the session-login check
- [x] Two error variants, one per refusal, with matching `ERRORS.md` headings — `SafetyModeTooLow { required }` and `DangerNotPermitted`
- [ ] Extend the debug identity shortcut to the new boundary
- [ ] Grade every handler on the administrative surface, module by module
- [ ] `just gen-openapi` step writing the generated grade map
- [ ] Sweep retiring idle sessions, in the jobs crate
- [ ] Session provider, session header in `callApi`, and mode indicator in the app bar
- [ ] Graded control wrappers carrying the stripe treatment
- [ ] Danger column on the administrators screen
- [ ] Boundary tests on the real header path, and Playwright coverage for the interface
