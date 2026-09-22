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

## How the grading went

204 operations: 98 read-only, 70 write, 36 danger.

Two readings settled most of the judgement calls, and are worth stating because
they are not quite on the face of the criteria.

**Time-bounded suppression is write; open-ended suppression is danger.** A
maintenance window announces itself, expires on its own, and can be lifted, so
declaring one is write. A silence has no expiry — it suppresses alerting until
somebody remembers to undo it — which is the same shape as the spec's "pausing
certificate renewal", so `silence_server`/`_machine`/`_group` and
`healthchecks::decommission` and `set_source_ingest` are danger. Their undo
handlers restore the protection, so those are write.

**"Cannot be undone from the interface" is about consequence to the fleet, not
about row permanence.** Deleting an incident note is permanent and is still
write; archiving a machine has no un-archive handler and cascades to the
applications on it, so it is danger.

The explicit examples in the spec landed where they should: deleting a backup
configuration, clearing a backup schedule, closing a machine's restore window,
revoking a certificate, pausing a server's certificate work, minting and revoking fleet-query tokens are danger;
creating a backup configuration and renaming a group are write; the SQL
playground and every fleet-query read are read-only.

## Calls worth a second look

`admins::add` and `admins::delete` are danger under "issues or invalidates
credentials or trust material": an allowlist entry is what admits a human to the
whole surface. The same reading makes every credential handler in `devices`
danger — provisioning, adding, deactivating, reactivating keys, changing a
device's role, attaching or detaching a tailnet identity, and merging records.

`backups::disallow_restore` is danger and `allow_restore` is write. The spec's
"disallowing a restore" example means closing a machine's restore window, and
the example now says so in the backup spec's own words.

Opening the window lets the machine mint restore credentials, which reads as
danger under the credentials criterion, but only an already-authenticated
machine can use it. The danger was spent upstream, where the machine was made
trusted: every handler that does that (`devices::provision_credential`,
`add_key`, `reactivate_key`, `update_role`, `attach_tailscale`, `merge_into`,
and `machines::mint_enrollment`, `attach_tailscale_device`) is danger. The spec
now states that rule, so the credentials criterion is not read as reaching it.

`backups::request_now` is danger because the same handler requests restores, and
a restore overwrites a live server. A backup alone would be write.

`inventory_variables::remove` is write, because the interface can set the
variable again. Where the variable is a secret its value is genuinely gone.

`backups::set_type_default` is write: it sets canopy-wide schedule and retention
defaults, so a shorter retention there eventually destroys backups fleet-wide,
but it amends Canopy's own records rather than acting on the fleet.

## Applying the blocked treatment across the surface

`GradedAction` wraps a control in the mode its endpoint requires, reading the
generated grade map so the stripe and the server's decision come from one
declaration. It is on the administrators screen; the rest of the surface's
controls are still to be wrapped.

That rollout is not only mechanical. The e2e stack runs a debug binary, so the
server skips the mode check while the interface holds a real read-only session:
a test that reaches for a graded control has to raise first, as an operator does.
`e2e/safety.ts` is that helper, and `admins.spec.ts` shows the shape. Every spec
that drives a control which becomes graded needs the same line, so the wrapping
and its tests move together, screen by screen.

## Build steps

- [x] Migration adding the danger column to `admins`, made with `just migration`
- [x] Migration adding the sessions table, and its model in the database crate
- [x] Grade type in the shared types crate, ordered so the ladder is a comparison — `commons_types::safety::SafetyMode`
- [x] Grant resolution returns both permission sets where it returns one today — `resolve_permissions`, `has_danger_by_policy`, `Admin::check_danger`/`set_danger`, `TailscaleUser::has_danger`
- [x] Grade argument and extension injection in the vendored `routes!` macro — `routes!(danger: delete)`, recorded as the `x-canopy-safety-mode` operation extension
- [x] Enforcement layer and middleware, including the session-login check — `private-server/src/safety.rs`, reading grades back from the built document
- [x] Two error variants, one per refusal, with matching `ERRORS.md` headings — `SafetyModeTooLow { required }` and `DangerNotPermitted`
- [x] Session endpoints (`/api/safety/session`, `raise`, `lower`), graded read-only so a read-only session can reach them
- [x] Extend the debug identity shortcut to the new boundary — `use_dev_identity()` made public and honoured by the middleware
- [x] Grade every handler on the administrative surface, module by module — all 204 operations across 28 modules
- [x] `just gen-openapi` step writing the generated grade map — `private-web/src/safety-modes.ts`, regenerated and diffed by `just check-generated`
- [x] Sweep retiring idle sessions, in the jobs crate — `jobs::session_sweep`, hourly, 24h grace, wired into the monitor pod
- [x] Session provider, session header in `callApi`, and mode indicator in the app bar
- [ ] Graded control wrappers carrying the stripe treatment — `GradedAction` built and applied on the administrators screen; the rest of the surface's controls still to be wrapped
- [x] Danger column on the administrators screen — `admins::list` carries the flag, `admins::set_danger` amends it
- [x] Boundary tests on the real header path, and Playwright coverage for the interface — `tests/it/safety_modes.rs` (9) and `e2e/safety-modes.spec.ts` (8)
