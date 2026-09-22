---
id: SAFE
---

# Safety modes

Every operator session on the private server's administrative surface runs in one of three safety modes, and every handler on that surface is graded to the mode it requires.
A session is read-only until the operator raises it, and returns to read-only on its own.
This governs what an operator may do at a given moment, and is distinct from whether they may reach the surface at all (see [ADM](admin-access.md)).

## The modes

The modes are read-only, write, and danger.
They are a ladder: danger permits everything write permits, and write permits everything read-only permits, so an operator in danger mode does ordinary write work without dropping back down.

A session begins read-only.
A raise lasts ten minutes from the moment it is made, and the session returns to read-only when that elapses, whatever the operator has been doing in the meantime.

The server accepts requests at a raised grade for one minute longer than the raise lasts.
That margin is not offered to the operator and does not appear anywhere: it exists so that a request already in flight as the raise ends is not refused for an expiry the operator had no way to anticipate.

## Raising and lowering

Raising to write takes effect without confirmation.
Raising to danger asks the operator to confirm first.

The raise control offers danger to every operator, because nothing tells a client in advance whether its operator holds the danger permission.
An operator who does not hold it is refused when they raise, and told that they lack the permission rather than that something went wrong.

An operator lowers their mode from the same control at any time, without waiting for the remaining time to run out.

## The session

A mode belongs to a single client session rather than to the operator's identity.
One operator working in two clients has two sessions, and those sessions can be in different modes.

The server holds each session's mode and decides every graded request against it, so a client reaches no further than its session's mode however it describes itself.
A session's mode holds however the surface is served, and survives a restart of it.

A client that has lost its record of its own mode is read-only, so doubt resolves downwards.
A request presenting a session identifier that is unknown or has expired is treated as read-only rather than refused outright.
A session is usable only by the login it belongs to.
A read-only-graded request needs no session at all, so a client can read before it has one.

A session that has not been seen for some time is retired.

## Grading the administrative surface

Every handler on the administrative surface declares the mode it requires, and a handler that declares none fails the build, so none can be added ungraded.
Each handler's grade is available to the operator interface, so that it can present a control according to the mode that control needs.

A handler that changes nothing is read-only.
This holds where the read is a sensitive one: the SQL playground runs operator-supplied queries inside a read-only transaction that is always rolled back, so it is graded read-only.

A handler that changes something is danger when any of the following holds, and write otherwise.

- It cannot be undone from the interface. Deleting a backup configuration qualifies; creating one does not.
- It acts on a production server rather than amending Canopy's own records. Revoking a certificate qualifies; renaming a server group does not.
- It removes a protection without destroying anything at the time. Disallowing a restore, pausing certificate renewal, and clearing a backup schedule all qualify.
- It issues or invalidates credentials or trust material. Minting a fleet-query access token and changing the certificate authority both qualify.

The fleet query interface (see [MCP](mcp.md)) changes nothing, so every one of its tools is read-only.
The handlers that mint, revoke, and list its access tokens belong to the administrative surface rather than to that interface: minting and revoking are danger, and listing is read-only.

A handler's grade is independent of who may reach it.
A handler reachable by a caller who is not an administrator is graded on the same basis as any other, and every operator has a safety mode.

## Refusals

A graded request from a session below the mode it requires is refused.
A danger-graded request from an operator who does not hold the danger permission (see [ADM](admin-access.md)) is refused whatever mode their session is in.

The two refusals are distinguishable to a client.
Lacking the permission says the operator can never make that request; being below the mode says they can, once they raise.
A client refused for its mode returns its indicator to read-only and tells the operator that their raise has lapsed, so client and server agree again without a reload.

## Presenting the mode

The current mode and the time remaining on it are visible at all times, counting down from ten minutes when a raise is made.

A reloaded page is read-only.

## Presenting blocked controls

A control the operator could use in a higher mode is present and blocked rather than removed, so the surface has the same shape whatever mode the operator is in.
A blocked control does not act when clicked, and names the mode it requires.
Raising is done from the mode control rather than as a by-product of reaching for a blocked control.

A control requiring write or danger carries the grade it requires as a diagonal stripe in that grade's colour, muted while at rest and coming to full colour under the pointer.
The same treatment is used throughout the surface, so an operator learns it once.

A control disabled for a reason unrelated to its grade, such as a request in flight or an incomplete form, carries no stripe, so the treatment never misreports why a control is unavailable.

A control the operator can never use, because they are not an administrator, is absent rather than blocked (see [ADM](admin-access.md)).
