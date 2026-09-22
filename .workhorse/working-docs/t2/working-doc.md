---
status: draft
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

The client treats a raise as lasting a minute less than the server does.
The server holds the raise for ten minutes; the client drops itself to read-only at nine.
So the client stops offering a control before the server would start refusing it, and an operator never has a request refused for an expiry they could not see coming.

The operator sees the client's window, because that is the one that governs what they can reach.
The countdown opens at nine minutes and the interface does not promise ten anywhere.

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

The grade rides the existing utoipa annotation on each handler, through `just gen-openapi`, into the generated wire types the interface reads.

Session identity is the one piece with no existing pattern to follow, since the surface authenticates purely from per-request tailnet headers today and holds no session of any kind.
It also has to work across several private-server processes, so wherever a session lives it is shared rather than per-process.

## Trade-offs

### Departing from seedling

The concept comes from seedling, but seedling's safety mode is advisory: its server records and broadcasts the mode each client reports and gates nothing on it.
Canopy enforces instead, because the danger permission has to be real and a mode the server already holds is the natural place to hang it.

The visual convention is taken from seedling deliberately and unchanged, so an operator who knows one interface knows the other.

Seedling's elevation window is 9m59s so that a countdown rounded to minutes opens at "10m".
Canopy's split of nine client-side against ten server-side is a different device for a different reason: seedling has nothing to be refused by.

## Open questions

- [ ] How a session mints and carries its identity on a request, which is the one piece with no existing pattern to follow

## Testing notes

- A read-only session is refused a write-graded request, and a write session is refused a danger-graded one.
- An operator without the danger permission cannot raise to danger, and is refused a danger-graded request even when their session claims danger mode.
- The two refusals are distinguishable, and a client meeting the not-raised one returns its indicator to read-only.
- A raise expires on its own, and the client stops offering graded controls a minute before the server stops accepting them.
- A raise survives being served by a different private-server process, and survives a restart.
- A reloaded page comes back read-only.
- Raising to danger asks for confirmation; raising to write does not.
- A blocked control does not act on a click, carries the stripe of the grade it needs, and names that grade.
- A control disabled for an unrelated reason carries no stripe.
- Every handler on the administrative surface has a grade, and one added without a grade fails the build.
- Every tool on the fleet query interface is read-only.
