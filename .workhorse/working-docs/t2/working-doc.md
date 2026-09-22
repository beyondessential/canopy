---
status: complete
---

# Gate the administrative surface behind safety modes

An operator's session runs in read-only, write, or danger mode; the mode gates every handler on the private server's administrative surface, is visible with its remaining time, and returns to read-only on its own.

## Behaviour

### The two layers

Two separate things are being built, and keeping them apart is what makes the card tractable.

**Safety mode is a guard rail, not a security boundary.**
Read-only by default, deliberately raised, self-lowering.
Its job is to stop the accidental click, not to stop a determined caller.

**The danger permission is a real permission, enforced server-side.**
It sits alongside the existing administrator permission on the same tailnet capability grant and the same allowlist table.
A danger-graded handler refuses an operator who does not hold it, whatever the client sends.

So grading every handler serves two ends at once: it tells the client which controls to block in which mode, and it marks the handlers that additionally carry the server-side danger check.

### The server holds the raise

A raise is registered with the server, and the server enforces it.
A write-graded handler refuses a request from a session that is not registered in write mode, so the guard rail is real rather than a convention a client could decline to follow.

Because a raise is scoped to a single client session rather than to the operator's identity, a session carries an identity of its own that it registers under and presents on every request.
One operator with two tabs is two sessions, and they can be in different modes.
This is new machinery on a surface that otherwise authenticates purely from per-request tailnet headers.

The mode itself is held by the client and reported to the session.
There is no probe that tells a client which permissions its operator holds: the client raises, and the server refuses if the operator may not.
A client that has lost its memory of its mode reports read-only, so any doubt resolves downwards.

Because the private server runs as several processes, a session is shared state rather than per-process memory, and a raise survives a restart of any one of them.

The server holds a raise for a minute longer than the client does.
The raise the operator sees lasts ten minutes and the countdown opens at ten; the server keeps accepting at that grade for eleven.
So a request already in flight as the raise ends is not refused for it, and an operator is never refused for an expiry they could not see coming.

The extra minute is slack, not a window: it is never shown and never offered.

### What this card does not carry

The registered sessions are what X3 reads to show that another operator is currently modifying things, and whereabouts in the surface they are.
That display, and the location reporting it needs, are sequenced after this card, which lands the registration itself.

Nothing about a raise or a danger-graded action is recorded.
Canopy has no systematic record of who did what, and building one belongs to U2, which covers the whole administrative surface at the middleware level rather than one grade of it.
The thing worth recording there is the danger-graded action, not an operator moving the toggle.

### Modes

The three modes are read-only, write, and danger.
They are a ladder.
Danger permits everything write permits, and write permits everything read-only permits, so an operator in danger mode can do ordinary write work without dropping back down.

A session starts read-only.
Raising to write happens without confirmation.
Raising to danger asks for confirmation.
The raise control offers danger to every operator, because no probe tells the client who holds the permission, so an operator without it learns so by trying and the refusal says plainly that they lack it rather than that something went wrong.

An operator lowers their mode deliberately from the same control, without waiting for the countdown.

A raised mode lasts ten minutes counted from the raise, and drops to read-only when that elapses regardless of what the operator has been doing in the meantime.
The current mode and its remaining time are visible at all times.

### Blocking, not hiding

A control the operator cannot presently use in their current mode is shown blocked rather than removed, so the interface has the same shape in every mode.
A blocked control does not respond to a click, and its tooltip names the mode it needs.
Raising stays a deliberate act performed from the mode indicator, rather than something that happens as a by-product of reaching for a control.

A control also carries the grade it requires, as a faint diagonal stripe in the colour of that grade: the warning colour for write, the error colour for danger.
The stripes are greyscaled at rest and come to full colour under the pointer, so a page at rest reads as ordinary and the grade reveals itself on hover.
The same convention is used throughout, so there is one thing to learn.
Seedling already uses exactly this treatment and canopy matches it exactly, so that an operator who knows one interface knows the other: 6px stripes on a 12px period, write at 135deg in the warning palette and danger at 45deg in the error palette, greyscaled 0.8 at rest and 0 on hover.

Striping is suppressed when a control is disabled for a reason unrelated to safety, such as a pending request or an invalid form, so the treatment never misreports why something is blocked.

The administrator boundary keeps hiding rather than blocking: a control an operator can never use in any mode is absent, as it is today.
Grading and administrator-ness are orthogonal, so a handler reachable by a non-administrator is graded on exactly the same basis as any other, and a non-administrator has a safety mode like anyone else.

### Grading a handler

Every handler declares its grade.
A handler without one does not compile, so all of them are graded once and a new handler cannot be added ungraded.
The grade is a visible, reviewable property of each handler rather than a list kept somewhere else.
It reaches the client through the generated OpenAPI document and the wire types generated from it, so a regrade travels to the interface by running the generation that already exists.
Only the private server's OpenAPI carries grades.

The grade follows from two questions, in order.

**Does the handler change anything?**
If not, it is read-only.
This holds even where the read is sensitive: the SQL playground runs arbitrary queries, but inside a read-only transaction that is always rolled back, so it is a read and it is graded read-only.
Listing MCP tokens is a read, so it is read-only; minting one is not.

**Does any of these apply?**
If so the handler is danger, and otherwise it is write.

- It is irreversible, in that the operator cannot undo it from the interface. Deleting a backup configuration qualifies; creating one does not.
- It acts on a production server rather than amending canopy's own records. Revoking a certificate qualifies; renaming a server group does not.
- It removes a safety net, so that nothing is destroyed at the time but a protection is lifted. Disallowing a restore, pausing certificate renewal, and clearing a backup schedule all qualify, and the damage they permit lands later, which is when nobody remembers the click.
- It touches credentials or trust material, issuing or invalidating what other things rely on. Minting an MCP token and changing the certificate authority both qualify, even though they create rather than destroy.

### Granting danger

Danger is carried on the same tailnet capability grant and the same allowlist as administrator, so there is one grant, one refresh, and one place to look.
The allowlist entry an operator manages records both permissions rather than membership alone, and the administrators screen is where danger is granted and withdrawn.

Administrator and danger are independent flags granted separately.
Danger without administrator reaches nothing today, because almost the whole surface is administrator-gated, so granting danger to an operator means granting both.

An operator who does not hold danger cannot enter danger mode, and the server refuses a danger-graded request from them whatever their client sends.

A refusal for lacking the danger permission and a refusal for not being raised to the required mode are different answers, and are distinguishable.
The first says the operator can never do this; the second says they can, once they raise.
A client meeting the second drops its indicator to read-only and tells the operator their raise has lapsed, so client and server reconverge without a reload.

### The query interface

The fleet query interface is read-only, so every one of its tools is graded read-only and nothing there is reachable above read-only.
The handlers that mint, revoke, and list its access tokens live on the administrative surface rather than the query interface, and are graded like anything else: minting and revoking touch trust material and are danger, listing is read-only.

## Implementation notes

### Declaring the grade

The grade is declared in each module's `routes()` table, in the vendored `routes!` macro: `routes!(danger: delete)` and so on.
Omitting it fails to match the macro arm, so an ungraded handler is a compile error without any extra machinery.

One declaration does three things: it registers the route as it does today, it injects the operation extension, and it attaches the grade to the route for enforcement.
Nothing is written twice, so nothing can drift.

The grade sits away from the handler body, which is the cost.
The gain is that each module's route table becomes a grading table a reviewer reads in one screen, which is what makes 130 judgements reviewable at all: `backups.rs` is 2600 lines and 28 handlers, and its grading is otherwise scattered through them.

Enforcement is a small layer that puts the grade in the request extensions, and one middleware that reads the grade, the session, and the identity, and decides.

### Two boundaries, two mechanisms

The administrator boundary stays exactly as it is: `TailscaleAdmin` resolves the identity, checks administrator status, and upserts the cached user for avatars.
The mode and danger checks are a separate layer.
Identity resolves twice per graded request as a result, which is the price of leaving a working administrator path untouched on a card that is already large.

The danger permission is re-resolved on every danger-graded request, against the allowlist and policy, exactly as the administrator check is today.
Withdrawing danger therefore takes effect at once rather than at the end of someone's raise.
The session records a mode; it never records a permission, and never grants anything by itself.

### The session

The server mints a session when a client connects, so a session always exists and a raise modifies the one already there.
A read-only session is the resting state, and read-only-graded handlers require no session at all, which matters because the first requests a page makes are in flight before the session exists.

A session is a row: its identifier, the login it belongs to, its mode, when the raise expires, and when it was last seen.
The identifier travels as a request header, added centrally where every call already goes through one function.

Two clocks, not one.
A raise expires ten minutes after it is made, and that is fixed; the server honours it for eleven.
A session's own liveness is separate and idle-based, since most sessions never raise and would otherwise accumulate forever.

A request updates its session's last-seen, and a periodic sweep retires sessions not seen for a while, alongside the domain and certificate sweeps that already run in the jobs crate.
Rows stay bounded, and X3 inherits a live-session list that is true rather than one padded with every tab ever opened.
If X3 wants a merely-open tab to count as present, it adds a heartbeat; nothing here forecloses that.

A request presenting an unknown or expired session identifier is treated as read-only rather than refused, because doubt resolves downwards.
The client meeting that refusal on a graded request starts a fresh session and returns its indicator to read-only.

A session's login is checked against the authenticated identity on every request, so an identifier belonging to someone else is not usable even though the tailnet makes that a remote concern.

### The debug identity, and 322 tests

This is the largest implementation risk on the card, and it is not in the feature.

Both tailnet extractors short-circuit in debug builds and return a fixed `admin@localhost`, so every debug build is an administrator for free.
The private-server suite has 322 test functions across 108 endpoint paths, and they POST straight at handlers with no session of any kind.
A mode layer that refuses an unraised request breaks nearly all of them at once.

So the debug shortcut extends to cover the new boundary: with the dev identity in play, a request is treated as holding a danger-mode session and as holding the danger permission, by skipping the checks rather than by seeding anything.
This mirrors exactly what the shortcut already does for administrator status, and is compiled out of release builds by the same guard, so no misconfiguration can reach it in production.
Existing tests are untouched.

Tests that exercise the boundary itself opt into the real path with `CANOPY_TRUST_TAILSCALE_HEADERS=1` and drive sessions explicitly, which is the pattern `admin_auth.rs` already sets and which card S2 built for precisely this.

### What changes where

- `admins` gains a danger column, through a migration made with `just migration`. The capability value grows a `danger` key beside `admin`, and grant resolution returns two sets where it returns one.
- A sessions table, and its model in the database crate.
- A grade type in the shared types crate, ordered so the ladder is a comparison.
- The vendored macro gains its grade argument and the extension injection.
- Two error variants, distinguishable as the behaviour requires: one for lacking the danger permission, one for not being raised. `ERRORS.md` gains a heading for each, matching the problem type.
- `just gen-openapi` gains the step that writes the generated grade map.
- The interface gains a session provider, a mode indicator in the app bar, graded control wrappers, and the danger column on the administrators screen.

Two naming collisions to avoid, both already in the tree: `OperatorPresence` and its `operator_presence.rs` test are the monitored-server feature, nothing to do with operators using canopy; and `ActionButton` is already taken by the icon button that reveals its label on hover, so the graded wrappers need their own name.

The session identifier has to be reachable from `callApi`, which is a bare function rather than a hook and is called from outside React.
So the provider publishes it to a module-level value that `callApi` reads, rather than threading it through every call site.

### Carrying the grade to the client

The grade lands in the OpenAPI document as an operation extension.
utoipa 5.5's `Operation` carries an `extensions` map for `x-` keys, so the grade travels with the operation it belongs to and no annotation is written twice.

`openapi-typescript` does not surface `x-` extensions in the types it generates, so the grade cannot arrive as a TypeScript type.
`just gen-openapi` gains a step that reads the emitted `openapi.json` and writes a generated module mapping each module and function to its grade.
That is the shape the interface wants anyway: `callApi(module, fn)` is a runtime pair, and a component choosing a stripe needs a runtime value rather than a type.
The generated file is committed alongside `openapi.json` and `api-types.ts`, as those already are.

### Why a session is a table

A signed token carrying login, mode, and expiry would need no storage, would work across processes, and would survive a restart.
It is rejected anyway, for two reasons that are decisions already made rather than preferences.

X3 reads the live sessions to show who is raised and where, and a token held only by its client cannot be enumerated.
An operator's raise also has to be able to end before its expiry, and a token cannot be withdrawn once issued.

So a session is a row: its own identifier, the login it belongs to, its mode, and when it expires.
That satisfies the shared-across-processes and survives-a-restart requirements for free, since Postgres is already the shared thing.

## Trade-offs

### Departing from seedling

The concept comes from seedling, but seedling's safety mode is advisory: its server records and broadcasts the mode each client reports and gates nothing on it.
Canopy enforces instead, because the danger permission has to be real and a mode the server already holds is the natural place to hang it.

The visual convention is taken from seedling deliberately and unchanged, so an operator who knows one interface knows the other.

Seedling's elevation window is 9m59s so that a countdown rounded to minutes opens at "10m".
Canopy's ten client-side against eleven server-side is a different device for a different reason: seedling's mode gates nothing, so it has nothing to be refused by and needs no slack.

## Open questions

None outstanding. The doc is ready to split.

## Testing notes

- A read-only session is refused a write-graded request, and a write session is refused a danger-graded one.
- An operator without the danger permission cannot raise to danger, and is refused a danger-graded request even when their session claims danger mode.
- The two refusals are distinguishable, and a client meeting the not-raised one returns its indicator to read-only.
- A raise expires on its own after ten minutes, and the server keeps accepting at that grade for a minute longer.
- A raise survives being served by a different private-server process, and survives a restart.
- A reloaded page comes back read-only.
- Raising to danger asks for confirmation; raising to write does not.
- A blocked control does not act on a click, carries the stripe of the grade it needs, and names that grade.
- A control disabled for an unrelated reason carries no stripe.
- Every handler on the administrative surface has a grade, and one added without a grade fails the build.
- Every tool on the fleet query interface is read-only.
- A read-only-graded handler answers a request carrying no session at all.
- A request carrying an unknown or expired session identifier is treated as read-only rather than refused outright.
- A session identifier is not usable by a login other than the one it belongs to.
- Withdrawing the danger permission takes effect during an operator's existing danger raise, not at the end of it.
- Sessions not seen for a while are retired, and a retired session no longer appears as live.
